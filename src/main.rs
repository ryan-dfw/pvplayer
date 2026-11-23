//! The `pvplay` binary: runs the player until an error occurs.
//!
//! See the [`pvplay`] library docs for how it works.

use pvplay::playback;
use std::process::ExitCode;

/// Runs [`playback::run`], printing its error (with context) and exiting
/// with a failure code if it stops.
fn main() -> ExitCode {
    match playback::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{:#}", e);
            ExitCode::FAILURE
        }
    }
}
