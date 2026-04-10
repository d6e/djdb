//! Mutation and query commands for the CLI. All commands load the dataset,
//! operate on it in memory, and write back any changed files. Writes go
//! through serde so formatting is regenerated from the in-memory model.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::NaiveDate;

use crate::data::Dataset;
use crate::performer::{Performer, Performers, Slug, is_valid_slug};
use crate::show::{Set, Show, WallTime};

// ---------- paths ----------

fn performers_path(data: &Path) -> PathBuf {
    data.join("performers.toml")
}

fn show_path(data: &Path, date: NaiveDate) -> PathBuf {
    data.join("shows").join(format!("{date}.toml"))
}

// ---------- io helpers ----------

fn save_performers(data: &Path, performers: &Performers) -> Result<()> {
    let path = performers_path(data);
    let text = toml::to_string_pretty(performers).context("serializing performers")?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn save_show(data: &Path, show: &Show) -> Result<()> {
    let shows_dir = data.join("shows");
    std::fs::create_dir_all(&shows_dir)?;
    let path = show_path(data, show.date);
    let text = toml::to_string_pretty(show).context("serializing show")?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn load_show_if_exists(data: &Path, date: NaiveDate) -> Result<Option<Show>> {
    let path = show_path(data, date);
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(Show::load(&path)?))
}

fn load_performers(data: &Path) -> Result<Performers> {
    Ok(Performers::load(&performers_path(data))?)
}

// ---------- add-set ----------

pub struct AddSetArgs<'a> {
    pub date: NaiveDate,
    pub start: WallTime,
    pub dj: Option<&'a str>,
    pub vj: Option<&'a str>,
    pub duration_min: u32,
    pub notes: &'a str,
}

pub fn add_set(data: &Path, args: AddSetArgs<'_>) -> Result<()> {
    if args.dj.is_none() && args.vj.is_none() {
        bail!("add-set requires at least one of --dj or --vj");
    }

    let performers = load_performers(data)?;
    for slug in [args.dj, args.vj].into_iter().flatten() {
        if !performers.contains(slug) {
            bail!(
                "unknown performer slug {slug:?} (create it first with `djdb new-performer`)"
            );
        }
    }

    let new_set = Set {
        start: args.start,
        duration_min: args.duration_min,
        dj: args.dj.map(str::to_string),
        vj: args.vj.map(str::to_string),
        notes: args.notes.to_string(),
    };

    let mut show = match load_show_if_exists(data, args.date)? {
        Some(s) => s,
        None => Show {
            date: args.date,
            sets: Vec::new(),
        },
    };

    // Reject exact-time collisions so we don't silently stack two sets on the
    // same start. Overlap detection (start..start+duration) is deliberately
    // out of scope: back-to-back 1h slots happen intentionally.
    if show
        .sets
        .iter()
        .any(|s| s.start == new_set.start)
    {
        bail!(
            "{date} already has a set starting at {h:02}:{m:02}",
            date = args.date,
            h = args.start.hour,
            m = args.start.minute
        );
    }

    show.sets.push(new_set);
    show.sets.sort_by_key(|s| (s.start.hour, s.start.minute));
    save_show(data, &show)?;
    Ok(())
}

// ---------- new-performer ----------

pub struct NewPerformerArgs<'a> {
    pub slug: &'a str,
    pub display_name: &'a str,
    pub twitch: &'a str,
    pub cdn: &'a str,
    pub notes: &'a str,
    pub aliases: Vec<String>,
}

pub fn new_performer(data: &Path, args: NewPerformerArgs<'_>) -> Result<()> {
    if !is_valid_slug(args.slug) {
        bail!(
            "invalid slug {slug:?}: use a-z, 0-9, and underscore only",
            slug = args.slug
        );
    }
    if args.display_name.trim().is_empty() {
        bail!("--name is required and must not be empty");
    }

    let mut performers = load_performers(data)?;
    if performers.contains(args.slug) {
        bail!("performer {slug:?} already exists", slug = args.slug);
    }

    performers.0.insert(
        args.slug.to_string(),
        Performer {
            display_name: args.display_name.trim().to_string(),
            aliases: args.aliases,
            notes: args.notes.to_string(),
            twitch: args.twitch.to_string(),
            cdn: args.cdn.to_string(),
        },
    );
    save_performers(data, &performers)?;
    Ok(())
}

// ---------- rename-performer ----------

pub fn rename_performer(data: &Path, old: &str, new: &str) -> Result<()> {
    if old == new {
        return Ok(());
    }
    if !is_valid_slug(new) {
        bail!("invalid new slug {new:?}");
    }

    let ds = Dataset::load(data)?;
    let Dataset {
        mut performers,
        shows,
    } = ds;

    let performer = performers
        .0
        .remove(old)
        .ok_or_else(|| anyhow!("performer {old:?} not found"))?;
    if performers.0.contains_key(new) {
        // Put it back and error.
        performers.0.insert(old.to_string(), performer);
        bail!("target slug {new:?} already exists (use `merge-performer` instead)");
    }
    performers.0.insert(new.to_string(), performer);

    // Rewrite every show that referenced the old slug.
    let mut rewritten = 0usize;
    for mut show in shows {
        let mut touched = false;
        for set in &mut show.sets {
            if set.dj.as_deref() == Some(old) {
                set.dj = Some(new.to_string());
                touched = true;
            }
            if set.vj.as_deref() == Some(old) {
                set.vj = Some(new.to_string());
                touched = true;
            }
        }
        if touched {
            save_show(data, &show)?;
            rewritten += 1;
        }
    }

    save_performers(data, &performers)?;
    eprintln!("renamed {old} -> {new} ({rewritten} shows updated)");
    Ok(())
}

// ---------- merge-performer ----------

pub fn merge_performer(data: &Path, from: &str, into: &str) -> Result<()> {
    if from == into {
        return Ok(());
    }
    let ds = Dataset::load(data)?;
    let Dataset {
        mut performers,
        shows,
    } = ds;

    let from_p = performers
        .0
        .remove(from)
        .ok_or_else(|| anyhow!("performer {from:?} not found"))?;
    let into_p = performers
        .0
        .get_mut(into)
        .ok_or_else(|| anyhow!("performer {into:?} not found"))?;

    // Fold `from`'s display name and aliases into `into`.
    if from_p.display_name != into_p.display_name
        && !into_p.aliases.contains(&from_p.display_name)
    {
        into_p.aliases.push(from_p.display_name.clone());
    }
    for alias in from_p.aliases {
        if alias != into_p.display_name && !into_p.aliases.contains(&alias) {
            into_p.aliases.push(alias);
        }
    }
    // Notes/links: keep into's, but append from's if non-empty and different.
    if !from_p.notes.is_empty() && from_p.notes != into_p.notes {
        if into_p.notes.is_empty() {
            into_p.notes = from_p.notes;
        } else {
            into_p.notes.push_str("\n---\n");
            into_p.notes.push_str(&from_p.notes);
        }
    }
    if into_p.twitch.is_empty() && !from_p.twitch.is_empty() {
        into_p.twitch = from_p.twitch;
    }
    if into_p.cdn.is_empty() && !from_p.cdn.is_empty() {
        into_p.cdn = from_p.cdn;
    }

    let mut rewritten = 0usize;
    for mut show in shows {
        let mut touched = false;
        for set in &mut show.sets {
            if set.dj.as_deref() == Some(from) {
                set.dj = Some(into.to_string());
                touched = true;
            }
            if set.vj.as_deref() == Some(from) {
                set.vj = Some(into.to_string());
                touched = true;
            }
        }
        if touched {
            save_show(data, &show)?;
            rewritten += 1;
        }
    }

    save_performers(data, &performers)?;
    eprintln!("merged {from} into {into} ({rewritten} shows updated)");
    Ok(())
}

// ---------- queries ----------

/// Map of slug -> most recent show date on which they played (dj or vj).
/// Performers with no sets do not appear in the map.
pub fn last_played_map(ds: &Dataset) -> HashMap<Slug, NaiveDate> {
    let mut out: HashMap<Slug, NaiveDate> = HashMap::new();
    for show in &ds.shows {
        for set in &show.sets {
            for slug in [set.dj.as_deref(), set.vj.as_deref()].into_iter().flatten() {
                out.entry(slug.to_string())
                    .and_modify(|d| {
                        if show.date > *d {
                            *d = show.date;
                        }
                    })
                    .or_insert(show.date);
            }
        }
    }
    out
}

pub fn last_played(data: &Path, slug: &str) -> Result<Option<NaiveDate>> {
    let ds = Dataset::load(data)?;
    if !ds.performers.contains(slug) {
        bail!("performer {slug:?} not found");
    }
    Ok(last_played_map(&ds).get(slug).copied())
}

/// Return (slug, last_played) for every performer whose last set is older
/// than `days` days ago (or who has never played). Sorted: never first,
/// then oldest date first.
pub fn stale(data: &Path, days: i64, today: NaiveDate) -> Result<Vec<(Slug, Option<NaiveDate>)>> {
    let ds = Dataset::load(data)?;
    let last = last_played_map(&ds);
    let cutoff = today - chrono::Duration::days(days);
    let mut out: Vec<(Slug, Option<NaiveDate>)> = Vec::new();
    for slug in ds.performers.0.keys() {
        match last.get(slug) {
            Some(&d) if d < cutoff => out.push((slug.clone(), Some(d))),
            None => out.push((slug.clone(), None)),
            _ => {}
        }
    }
    out.sort_by_key(|(_, d)| match d {
        None => (0i64, String::new()),
        Some(d) => (1, d.to_string()),
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup(dir: &Path) {
        std::fs::create_dir_all(dir.join("shows")).unwrap();
        std::fs::write(
            dir.join("performers.toml"),
            r#"
[aliquem]
display_name = "Aliquem"

[kohada]
display_name = "Kohada"

[turels]
display_name = "Turels"
"#,
        )
        .unwrap();
    }

    fn show_with_set(dir: &Path, date: &str, time: &str, dj: &str) {
        let path = dir.join("shows").join(format!("{date}.toml"));
        std::fs::write(
            &path,
            format!(
                r#"date = "{date}"

[[sets]]
start = "{time}"
dj = "{dj}"
"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn add_set_creates_new_show_file() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        add_set(
            dir.path(),
            AddSetArgs {
                date: NaiveDate::from_ymd_opt(2026, 5, 9).unwrap(),
                start: WallTime { hour: 21, minute: 0 },
                dj: Some("aliquem"),
                vj: None,
                duration_min: 60,
                notes: "",
            },
        )
        .unwrap();
        // Validates end-to-end.
        let ds = Dataset::load(dir.path()).unwrap();
        assert_eq!(ds.shows.len(), 1);
        assert_eq!(ds.shows[0].sets.len(), 1);
    }

    #[test]
    fn add_set_appends_and_sorts() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        show_with_set(dir.path(), "2026-05-09", "22:00", "kohada");
        add_set(
            dir.path(),
            AddSetArgs {
                date: NaiveDate::from_ymd_opt(2026, 5, 9).unwrap(),
                start: WallTime { hour: 21, minute: 0 },
                dj: Some("aliquem"),
                vj: None,
                duration_min: 60,
                notes: "",
            },
        )
        .unwrap();
        let show = load_show_if_exists(
            dir.path(),
            NaiveDate::from_ymd_opt(2026, 5, 9).unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(show.sets.len(), 2);
        assert_eq!(show.sets[0].start.hour, 21);
        assert_eq!(show.sets[1].start.hour, 22);
    }

    #[test]
    fn add_set_rejects_unknown_slug() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        let err = add_set(
            dir.path(),
            AddSetArgs {
                date: NaiveDate::from_ymd_opt(2026, 5, 9).unwrap(),
                start: WallTime { hour: 21, minute: 0 },
                dj: Some("ghost"),
                vj: None,
                duration_min: 60,
                notes: "",
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    fn add_set_rejects_time_collision() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        show_with_set(dir.path(), "2026-05-09", "21:00", "kohada");
        let err = add_set(
            dir.path(),
            AddSetArgs {
                date: NaiveDate::from_ymd_opt(2026, 5, 9).unwrap(),
                start: WallTime { hour: 21, minute: 0 },
                dj: Some("aliquem"),
                vj: None,
                duration_min: 60,
                notes: "",
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("already has a set"));
    }

    #[test]
    fn new_performer_creates_entry() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        new_performer(
            dir.path(),
            NewPerformerArgs {
                slug: "pizza_princess",
                display_name: "Pizza Princess",
                twitch: "https://twitch.tv/pp",
                cdn: "",
                notes: "",
                aliases: vec![],
            },
        )
        .unwrap();
        let p = load_performers(dir.path()).unwrap();
        let pp = p.get("pizza_princess").unwrap();
        assert_eq!(pp.display_name, "Pizza Princess");
        assert_eq!(pp.twitch, "https://twitch.tv/pp");
    }

    #[test]
    fn new_performer_rejects_existing() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        let err = new_performer(
            dir.path(),
            NewPerformerArgs {
                slug: "aliquem",
                display_name: "Aliquem",
                twitch: "",
                cdn: "",
                notes: "",
                aliases: vec![],
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn new_performer_rejects_bad_slug() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        let err = new_performer(
            dir.path(),
            NewPerformerArgs {
                slug: "Pizza Princess",
                display_name: "Pizza Princess",
                twitch: "",
                cdn: "",
                notes: "",
                aliases: vec![],
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid slug"));
    }

    #[test]
    fn rename_performer_updates_shows() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        show_with_set(dir.path(), "2026-05-09", "21:00", "aliquem");
        show_with_set(dir.path(), "2026-05-16", "22:00", "aliquem");
        rename_performer(dir.path(), "aliquem", "ali").unwrap();
        let ds = Dataset::load(dir.path()).unwrap();
        assert!(ds.performers.contains("ali"));
        assert!(!ds.performers.contains("aliquem"));
        for show in &ds.shows {
            for set in &show.sets {
                assert_ne!(set.dj.as_deref(), Some("aliquem"));
            }
        }
    }

    #[test]
    fn rename_performer_rejects_target_exists() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        let err = rename_performer(dir.path(), "aliquem", "kohada").unwrap_err();
        assert!(err.to_string().contains("already exists"));
        // Original must still exist.
        let p = load_performers(dir.path()).unwrap();
        assert!(p.contains("aliquem"));
    }

    #[test]
    fn merge_performer_folds_aliases_and_rewrites() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        show_with_set(dir.path(), "2026-05-09", "21:00", "turels");
        merge_performer(dir.path(), "turels", "aliquem").unwrap();
        let ds = Dataset::load(dir.path()).unwrap();
        assert!(!ds.performers.contains("turels"));
        let ali = ds.performers.get("aliquem").unwrap();
        assert!(ali.aliases.iter().any(|a| a == "Turels"));
        for show in &ds.shows {
            for set in &show.sets {
                assert_ne!(set.dj.as_deref(), Some("turels"));
            }
        }
    }

    #[test]
    fn last_played_returns_latest() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        show_with_set(dir.path(), "2026-05-09", "21:00", "aliquem");
        show_with_set(dir.path(), "2026-05-16", "21:00", "aliquem");
        show_with_set(dir.path(), "2026-05-02", "21:00", "aliquem");
        let d = last_played(dir.path(), "aliquem").unwrap();
        assert_eq!(d, Some(NaiveDate::from_ymd_opt(2026, 5, 16).unwrap()));
    }

    #[test]
    fn last_played_never() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        show_with_set(dir.path(), "2026-05-09", "21:00", "aliquem");
        let d = last_played(dir.path(), "kohada").unwrap();
        assert_eq!(d, None);
    }

    #[test]
    fn stale_lists_never_and_old() {
        let dir = tempdir().unwrap();
        setup(dir.path());
        // aliquem played recently, kohada played long ago, turels never.
        show_with_set(dir.path(), "2026-05-01", "21:00", "aliquem");
        show_with_set(dir.path(), "2025-01-01", "21:00", "kohada");
        let today = NaiveDate::from_ymd_opt(2026, 5, 9).unwrap();
        let out = stale(dir.path(), 60, today).unwrap();
        // Never-played comes first, then kohada, aliquem excluded.
        let slugs: Vec<&str> = out.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(slugs, vec!["turels", "kohada"]);
        assert_eq!(out[0].1, None);
        assert_eq!(out[1].1, Some(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()));
    }
}
