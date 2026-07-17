use std::io::{BufReader, BufWriter};
use std::process::ExitCode;

use pop_language_server::{ExitStatus, TransportLimits, serve};

fn main() -> ExitCode {
    let input = std::io::stdin();
    let output = std::io::stdout();
    match serve(
        BufReader::new(input.lock()),
        BufWriter::new(output.lock()),
        TransportLimits::default(),
    ) {
        Ok(ExitStatus::Success) => ExitCode::SUCCESS,
        Ok(ExitStatus::Failure) | Err(_) => ExitCode::FAILURE,
    }
}
