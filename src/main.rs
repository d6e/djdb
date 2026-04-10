use std::path::PathBuf;
use std::process::ExitCode;

use chrono::NaiveDate;
use clap::{Parser, Subcommand};

use djdb::data::Dataset;
use djdb::import::{build_plan, write_plan};

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
    /// Import from a legacy Google Sheets .xlsx workbook.
    Import {
        /// Path to the .xlsx file.
        xlsx: PathBuf,
        /// The real date of the last sheet in the workbook (year anchor).
        #[arg(long, default_value = "2026-05-02")]
        anchor: NaiveDate,
        /// Drop shows whose inferred year is earlier than this (use the
        /// dry-run histogram to pick a cutoff that excludes misordered
        /// early sheets).
        #[arg(long)]
        min_year: Option<i32>,
        /// Print what would be written without touching disk.
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Check => {
            let ds = Dataset::load(&cli.data)?;
            println!(
                "ok: {} performers, {} shows",
                ds.performers.0.len(),
                ds.shows.len()
            );
            Ok(())
        }
        Command::Import {
            xlsx,
            anchor,
            min_year,
            dry_run,
        } => {
            use chrono::Datelike;
            let mut plan = build_plan(&xlsx, anchor)?;
            if let Some(min_year) = min_year {
                let before = plan.shows.len();
                plan.shows.retain(|s| s.date.year() >= min_year);
                let dropped = before - plan.shows.len();
                // Rebuild performer set to drop anyone no longer referenced.
                let mut kept_slugs: std::collections::BTreeSet<String> =
                    std::collections::BTreeSet::new();
                for s in &plan.shows {
                    for set in &s.show.sets {
                        if let Some(slug) = &set.dj {
                            kept_slugs.insert(slug.clone());
                        }
                        if let Some(slug) = &set.vj {
                            kept_slugs.insert(slug.clone());
                        }
                    }
                }
                plan.performers.0.retain(|k, _| kept_slugs.contains(k));
                println!(
                    "filtered: dropped {dropped} shows before {min_year}, pruned performers to {}",
                    plan.performers.0.len()
                );
            }
            print_plan_summary(&plan);
            if dry_run {
                println!("\ndry-run: no files written");
            } else {
                write_plan(&plan, &cli.data)?;
                println!("\nwrote {} shows to {}", plan.shows.len(), cli.data.display());
            }
            Ok(())
        }
    }
}

fn print_plan_summary(plan: &djdb::import::ImportPlan) {
    use chrono::Datelike;
    use std::collections::BTreeMap;

    println!("=== import plan ===");
    println!("  performers: {}", plan.performers.0.len());
    println!("  shows:      {}", plan.shows.len());
    if !plan.shows.is_empty() {
        let first = plan.shows.first().unwrap();
        let last = plan.shows.last().unwrap();
        println!("  date range: {} .. {}", first.date, last.date);

        let mut by_year: BTreeMap<i32, usize> = BTreeMap::new();
        for s in &plan.shows {
            *by_year.entry(s.date.year()).or_insert(0) += 1;
        }
        println!("  shows per year:");
        for (year, n) in &by_year {
            println!("    {year}: {n}");
        }
    }
    if !plan.skipped.is_empty() {
        println!("\n  skipped {} sheets:", plan.skipped.len());
        for (name, reason) in &plan.skipped {
            println!("    [{name}] {reason}");
        }
    }
    if !plan.merges.is_empty() {
        println!("\n  {} alias merges:", plan.merges.len());
        for (slug, alias) in &plan.merges {
            println!("    {slug} <- {alias:?}");
        }
    }
}
