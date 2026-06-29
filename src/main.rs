#![windows_subsystem = "windows"]

mod assuan;
mod dialog;
mod error;
mod state;

use std::io::{BufReader, BufWriter};

fn main() {
    // Log to stderr only; stdout is the Assuan protocol pipe.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .target(env_logger::Target::Stderr)
        .init();

    if let Err(e) = run() {
        log::error!("Fatal error: {}", e);
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let stdin = BufReader::new(std::io::stdin());
    let stdout = BufWriter::new(std::io::stdout());
    let mut state = state::PinentryState::new();
    assuan::run_loop(stdin, stdout, &mut state)
}
