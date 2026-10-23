//! Native process fixture for supervisor tests on Unix and Windows.
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Execute one synthetic worker behavior without a platform shell.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args[1].as_str() {
        "clean" => print!("clean\n"),
        "registers" => print!("registers\n"),
        "exit" => {
            print!("clean\n");
            std::io::stdout().flush().unwrap();
            std::process::exit(3);
        }
        "start" => std::process::exit(4),
        "empty" => {}
        "invalid" => std::io::stdout().write_all(&[0xff]).unwrap(),
        "flood" => {
            let _ = std::io::stdout().write_all(&vec![b'x'; 1_000_000]);
        }
        "bound" => std::io::stdout().write_all(&[b'x'; 256]).unwrap(),
        "oversized" => std::io::stdout().write_all(&[b'x'; 257]).unwrap(),
        "sleep" => std::thread::sleep(Duration::from_secs(2)),
        "descendant" => {
            let _child = Command::new(std::env::current_exe().unwrap())
                .arg("hold")
                .arg(&args[2])
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            print!("clean\n");
        }
        "hold" => {
            std::thread::sleep(Duration::from_secs(2));
            std::fs::write(&args[2], "survived").unwrap();
        }
        _ => std::process::exit(9),
    }
}
