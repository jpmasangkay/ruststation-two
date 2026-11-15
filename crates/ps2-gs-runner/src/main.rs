//! CLI entrypoint for `ps2-gs-runner`.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::Parser;
use ps2_gs_runner::{run_cli, Cli};

fn main() -> ExitCode {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match run_cli(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ps2-gs-runner error: {err}");
            ExitCode::FAILURE
        }
    }
}
