use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use djdb::data::Dataset;

#[derive(Parser)]
#[command(name = "djdb", about = "KaleidoSky lineup database")]
struct Cli {
    /// Path to the data directory.
    #[arg(long, default_value = "data")]
    data: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate all performer and show files.
    Check,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Check => match Dataset::load(&cli.data) {
            Ok(ds) => {
                println!(
                    "ok: {} performers, {} shows",
                    ds.performers.0.len(),
                    ds.shows.len()
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
    }
}
