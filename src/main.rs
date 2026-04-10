use std::path::PathBuf;
use std::process::ExitCode;

use chrono::NaiveDate;
use clap::{Parser, Subcommand};

use djdb::commands::{self, AddSetArgs, NewPerformerArgs};
use djdb::data::Dataset;
use djdb::import::{build_plan, write_plan};
use djdb::show::WallTime;

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
    /// Append a set to a show (creates the show file if it doesn't exist).
    AddSet {
        /// Show date (YYYY-MM-DD), anchored to ET.
        date: NaiveDate,
        /// Set start time as HH:MM in ET. Hours 24..47 denote the morning
        /// after the show date.
        time: String,
        /// DJ slug.
        #[arg(long)]
        dj: Option<String>,
        /// VJ slug.
        #[arg(long)]
        vj: Option<String>,
        /// Duration in minutes.
        #[arg(long, default_value_t = 60)]
        duration: u32,
        /// Notes (twitch/cdn/etc).
        #[arg(long, default_value = "")]
        notes: String,
    },
    /// Create a new performer entry.
    NewPerformer {
        /// Slug (a-z, 0-9, underscore).
        slug: String,
        /// Display name.
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        twitch: String,
        #[arg(long, default_value = "")]
        cdn: String,
        #[arg(long, default_value = "")]
        notes: String,
        /// Alternate spelling (may be repeated).
        #[arg(long = "alias")]
        aliases: Vec<String>,
    },
    /// Rename a performer everywhere.
    RenamePerformer { old: String, new: String },
    /// Merge one performer into another, folding aliases and rewriting sets.
    MergePerformer { from: String, into: String },
    /// Show the most recent date a performer played.
    LastPlayed { slug: String },
    /// List performers not seen recently (or never).
    Stale {
        /// Days threshold; anyone whose last set is older than this appears.
        #[arg(long, default_value_t = 60)]
        days: i64,
    },
    /// Build the static site into `docs/`.
    Build {
        /// Output directory.
        #[arg(long, default_value = "docs")]
        out: PathBuf,
        /// URL prefix for links (use `/djdb/` for a GitHub project site).
        #[arg(long, default_value = "/")]
        base_url: String,
    },
    /// Import from a legacy Google Sheets .xlsx workbook.
    Import {
        /// Path to the .xlsx file.
        xlsx: PathBuf,
        /// The real date of the last sheet in the workbook (year anchor).
        #[arg(long, default_value = "2026-05-02")]
        anchor: NaiveDate,
        /// Drop shows whose inferred year is earlier than this.
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
        Command::AddSet {
            date,
            time,
            dj,
            vj,
            duration,
            notes,
        } => {
            let start = WallTime::parse(&time)?;
            commands::add_set(
                &cli.data,
                AddSetArgs {
                    date,
                    start,
                    dj: dj.as_deref(),
                    vj: vj.as_deref(),
                    duration_min: duration,
                    notes: &notes,
                },
            )?;
            println!("added set to {date}");
            Ok(())
        }
        Command::NewPerformer {
            slug,
            name,
            twitch,
            cdn,
            notes,
            aliases,
        } => {
            commands::new_performer(
                &cli.data,
                NewPerformerArgs {
                    slug: &slug,
                    display_name: &name,
                    twitch: &twitch,
                    cdn: &cdn,
                    notes: &notes,
                    aliases,
                },
            )?;
            println!("created performer {slug}");
            Ok(())
        }
        Command::RenamePerformer { old, new } => {
            commands::rename_performer(&cli.data, &old, &new)
        }
        Command::MergePerformer { from, into } => {
            commands::merge_performer(&cli.data, &from, &into)
        }
        Command::LastPlayed { slug } => {
            match commands::last_played(&cli.data, &slug)? {
                Some(d) => println!("{slug}: {d}"),
                None => println!("{slug}: never"),
            }
            Ok(())
        }
        Command::Stale { days } => {
            let today = chrono::Local::now().date_naive();
            let rows = commands::stale(&cli.data, days, today)?;
            if rows.is_empty() {
                println!("no stale performers (threshold {days} days)");
            } else {
                println!("{} stale performers (threshold {days} days):", rows.len());
                for (slug, last) in rows {
                    match last {
                        Some(d) => {
                            let ago = (today - d).num_days();
                            println!("  {slug}  last: {d} ({ago} days ago)");
                        }
                        None => println!("  {slug}  last: never"),
                    }
                }
            }
            Ok(())
        }
        Command::Build { out, base_url } => {
            let ds = Dataset::load(&cli.data)?;
            let cfg = djdb::site::BuildConfig {
                base_url,
                today: chrono::Local::now().date_naive(),
            };
            djdb::site::build(&ds, &cfg, &out)?;
            println!(
                "built {} shows + {} performers to {}",
                ds.shows.len(),
                ds.performers.0.len(),
                out.display()
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
