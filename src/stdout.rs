//! Stdout pipe closure is normal consumption; other write failures remain errors.
use std::io::{self, Write};

/// Stops writing after a consumer closes its pipe, while other report sinks can finish.
pub struct PipeWriter<W> {
    inner: W,
    closed: bool,
}

impl<W> PipeWriter<W> {
    /// Wrap only the stdout sink, never file reports or stderr.
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            closed: false,
        }
    }

    /// Inspect the underlying stream's terminal capability.
    pub fn inner(&self) -> &W {
        &self.inner
    }
}

impl<W: Write> Write for PipeWriter<W> {
    /// Treat a closed consumer as having consumed the rest of the report.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.closed {
            return Ok(bytes.len());
        }
        match self.inner.write(bytes) {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                self.closed = true;
                Ok(bytes.len())
            }
            result => result,
        }
    }

    /// Flush a live stream and retain all failures except a closed pipe.
    fn flush(&mut self) -> io::Result<()> {
        if self.closed {
            return Ok(());
        }
        match self.inner.flush() {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                self.closed = true;
                Ok(())
            }
            result => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Failing(io::ErrorKind);
    impl Write for Failing {
        /// Return the selected write failure for contract tests.
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(self.0.into())
        }
        /// Return the selected flush failure for contract tests.
        fn flush(&mut self) -> io::Result<()> {
            Err(self.0.into())
        }
    }

    /// Ordinary data survives and only BrokenPipe is ignored on writes and flushes.
    #[test]
    fn only_broken_pipes_are_ignored() {
        let mut normal = PipeWriter::new(Vec::new());
        normal.write_all(b"report").unwrap();
        normal.flush().unwrap();
        assert_eq!(normal.inner(), b"report");
        // Once either operation observes closure, later writes/flushes never touch the sink.
        for close_on_flush in [false, true] {
            let mut closed = PipeWriter::new(Failing(io::ErrorKind::BrokenPipe));
            if close_on_flush {
                closed.flush().unwrap();
            } else {
                closed.write_all(b"first").unwrap();
            }
            closed.inner.0 = io::ErrorKind::Other;
            closed.write_all(b"remaining report").unwrap();
            closed.flush().unwrap();
        }
        for kind in [
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::Other,
        ] {
            assert_eq!(
                PipeWriter::new(Failing(kind)).write_all(b"report").is_ok(),
                kind == io::ErrorKind::BrokenPipe
            );
            assert_eq!(
                PipeWriter::new(Failing(kind)).flush().is_ok(),
                kind == io::ErrorKind::BrokenPipe
            );
        }
    }
}
