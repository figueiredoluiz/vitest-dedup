//! Deadline-bound collection of a package worker's complete response.

use crate::model::PackageWorkerFailure as Failure;
use std::time::{Duration, Instant};
use subprocess::{Exec, Job, Redirection};

/// Largest worker response retained, with one extra byte read to detect overflow.
const MAX_OUTPUT: usize = 256;

/// Time left to observe completion. Equality is already a timeout.
fn remaining(deadline: Duration, elapsed: Duration) -> Result<Duration, Failure> {
    deadline
        .checked_sub(elapsed)
        .filter(|left| !left.is_zero())
        .ok_or(Failure::Timeout)
}

/// Kill the worker and, on Unix, its process group before reaping it. The group leader is
/// deliberately not polled while stdout is open, so a departed leader's descendants remain
/// reachable through its unreaped PID. Cleanup has its own bounded wait if termination fails.
fn stop(job: &Job) {
    #[cfg(unix)]
    {
        use subprocess::unix::JobExt;
        let _ = job.send_signal_group(libc::SIGKILL);
    }
    let _ = job.kill();
    let _ = job.wait_timeout(Duration::from_secs(1));
}

/// Read stdout under the same deadline as process completion, with bounded allocation.
/// A descendant that keeps stdout open cannot prolong the read. Unix failures terminate the
/// worker's process group; Windows terminates the worker and cancels pending pipe I/O.
pub(super) fn supervise(command: Exec, deadline: Duration) -> Result<String, Failure> {
    let started = Instant::now();
    supervise_with_clock(command, deadline, || started.elapsed())
}

/// The injected monotonic elapsed time makes late-completion ordering testable without sleeps.
fn supervise_with_clock(
    command: Exec,
    deadline: Duration,
    elapsed: impl Fn() -> Duration,
) -> Result<String, Failure> {
    remaining(deadline, elapsed())?;
    #[cfg(unix)]
    let command = {
        use subprocess::ExecExt;
        command.setpgid()
    };
    let mut job = command
        .stdin(Redirection::Null)
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Null)
        .detached()
        .start()
        .map_err(|_| Failure::Start)?;
    let answer = (|| {
        let left = remaining(deadline, elapsed())?;
        let (out, _) = job
            .communicate()
            .map_err(|_| Failure::Io)?
            .limit_size(MAX_OUTPUT + 1)
            .limit_time(left)
            .read()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::TimedOut {
                    Failure::Timeout
                } else {
                    Failure::Io
                }
            })?;
        let left = remaining(deadline, elapsed())?;
        if out.len() > MAX_OUTPUT {
            return Err(Failure::Output);
        }
        let status = job
            .wait_timeout(left)
            .map_err(|_| Failure::Io)?
            .ok_or(Failure::Timeout)?;
        remaining(deadline, elapsed())?;
        if !status.success() {
            return Err(if status.code() == Some(4) {
                Failure::Start
            } else {
                Failure::Exit
            });
        }
        String::from_utf8(out).map_err(|_| Failure::Output)
    })();
    if answer.is_err() {
        stop(&job);
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        OnceLock,
    };

    /// Build the same tiny native worker fixture on every supported platform, once per test run.
    fn probe(mode: &str) -> Exec {
        static PROBE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
        let (_, executable) = PROBE.get_or_init(|| {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("probe.rs");
            std::fs::write(
                &source,
                include_str!("../../tests/support/package_worker_probe.rs"),
            )
            .unwrap();
            let executable = directory
                .path()
                .join(format!("probe{}", std::env::consts::EXE_SUFFIX));
            let output = std::process::Command::new(
                std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()),
            )
            .arg("--edition=2021")
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            (directory, executable)
        });
        Exec::cmd(executable).arg(mode)
    }

    /// Exercise successful verdicts, bad exits, byte bounds and invalid UTF-8 through real pipes.
    #[test]
    fn supervisor_handles_complete_and_failed_children() {
        let deadline = Duration::from_secs(10);
        assert_eq!(supervise(probe("clean"), deadline), Ok("clean\n".into()));
        assert_eq!(
            supervise(probe("registers"), deadline),
            Ok("registers\n".into())
        );
        assert_eq!(supervise(probe("empty"), deadline), Ok(String::new()));
        assert_eq!(supervise(probe("exit"), deadline), Err(Failure::Exit));
        assert_eq!(supervise(probe("start"), deadline), Err(Failure::Start));
        assert_eq!(supervise(probe("invalid"), deadline), Err(Failure::Output));
        assert_eq!(supervise(probe("bound"), deadline), Ok("x".repeat(256)));
        assert_eq!(
            supervise(probe("oversized"), deadline),
            Err(Failure::Output)
        );
        assert_eq!(supervise(probe("flood"), deadline), Err(Failure::Output));
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            supervise(Exec::cmd(directory.path().join("missing")), deadline),
            Err(Failure::Start)
        );
    }

    /// Both an active worker and an exited worker's inherited pipe are subject to the deadline.
    #[test]
    fn deadline_covers_process_and_descendant_stdout() {
        let command = probe("sleep");
        let started = Instant::now();
        assert_eq!(
            supervise(command, Duration::from_millis(100)),
            Err(Failure::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("descendant-finished");
        let started = Instant::now();
        assert_eq!(
            supervise(probe("descendant").arg(&marker), Duration::from_millis(200)),
            Err(Failure::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        // The Windows contract bounds the read and kills the worker, but does not promise a job
        // object for descendants. Give the short-lived fixture time to finish on both platforms.
        std::thread::sleep(Duration::from_millis(2100));
        #[cfg(unix)]
        assert!(
            !marker.exists(),
            "the worker's process group survived cleanup"
        );
    }

    /// A clean, successful exit observed at the deadline must not be accepted, even if reads
    /// and waits completed successfully; the injected clock makes that ordering deterministic.
    #[test]
    fn completed_answer_at_deadline_is_refused() {
        let command = probe("clean");
        let deadline = Duration::from_secs(10);
        let ticks = AtomicUsize::new(0);
        assert_eq!(
            supervise_with_clock(command, deadline, || {
                if ticks.fetch_add(1, Ordering::Relaxed) < 3 {
                    Duration::ZERO
                } else {
                    deadline
                }
            }),
            Err(Failure::Timeout)
        );
        assert_eq!(ticks.load(Ordering::Relaxed), 4);
        assert_eq!(
            remaining(deadline, deadline + Duration::from_nanos(1)),
            Err(Failure::Timeout)
        );
    }
}
