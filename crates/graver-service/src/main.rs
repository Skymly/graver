use graver_engine::Engine;
use graver_ipc::PIPE_NAME;
use graver_service::{ServiceError, serve_pipe, serve_stream};

fn main() {
    if let Err(err) = run() {
        eprintln!("graver-service: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), ServiceError> {
    match std::env::args().nth(1).as_deref() {
        Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        Some("--version") => {
            println!("graver-service {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--stdio") => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            serve_stream(&mut Engine::new(), stdin.lock(), stdout.lock())
        }
        Some("--pipe") | None => serve_pipe(PIPE_NAME),
        Some(other) => {
            eprintln!("unknown argument: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "\
Graver service {version}

Usage:
  graver-service            Listen on {pipe}
  graver-service --stdio    Read the same frames from stdin
  graver-service --help

Each connection has its own composition session. The current schema only
buffers characters. This process does not register an input method.
",
        version = env!("CARGO_PKG_VERSION"),
        pipe = PIPE_NAME
    );
}
