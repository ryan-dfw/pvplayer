use pvplay::playback;
use std::process::ExitCode;

fn main() -> ExitCode {
    match playback::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{:#}", e);
            ExitCode::FAILURE
        }
    }
}