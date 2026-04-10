//! One-shot importer for the legacy Google Sheets workbook.
//!
//! Reads every sheet (hidden or not) from an .xlsx file, extracts lineups,
//! infers years by walking sheets in workbook order backward from an anchor
//! date, slugifies performer names, and produces an `ImportPlan` that can be
//! printed (dry-run) or written to disk.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use calamine::{Data, Reader, open_workbook_auto};
use chrono::{Datelike, NaiveDate};
use deunicode::deunicode;

use crate::performer::{Performer, Performers, Slug};
use crate::show::{Set, Show, WallTime};

/// A single sheet parsed into raw form before year assignment.
#[derive(Debug)]
struct RawSheet {
    sheet_name: String,
    month: u32,
    day: u32,
    rows: Vec<RawRow>,
}

#[derive(Debug)]
struct RawRow {
    parsed_time: Option<(u8, u8)>,
    dj: Option<String>,
    vj: Option<String>,
    notes: String,
}

/// Result of planning an import: ready to print or write.
#[derive(Debug, Default)]
pub struct ImportPlan {
    pub shows: Vec<PlannedShow>,
    pub performers: Performers,
    /// Sheets skipped, with reason.
    pub skipped: Vec<(String, String)>,
    /// Slug collisions: display names that were merged as aliases.
    pub merges: Vec<(Slug, String)>,
}

#[derive(Debug)]
pub struct PlannedShow {
    pub date: NaiveDate,
    pub source_sheet: String,
    pub show: Show,
}

/// Build the import plan.
///
/// `anchor`: the date of the last sheet in workbook order. Years are assigned
/// by walking backward and decrementing whenever month-day jumps upward.
pub fn build_plan(xlsx_path: &Path, anchor: NaiveDate) -> Result<ImportPlan> {
    let mut wb = open_workbook_auto(xlsx_path)
        .with_context(|| format!("opening {}", xlsx_path.display()))?;
    let sheet_names: Vec<String> = wb.sheet_names().to_vec();

    let mut raws: Vec<Option<RawSheet>> = Vec::with_capacity(sheet_names.len());
    let mut plan = ImportPlan::default();

    for name in &sheet_names {
        let range = wb
            .worksheet_range(name)
            .with_context(|| format!("reading sheet {name:?}"))?;
        match parse_sheet(name, &range) {
            Ok(raw) => raws.push(Some(raw)),
            Err(reason) => {
                plan.skipped.push((name.clone(), reason));
                raws.push(None);
            }
        }
    }

    // Year inference: walk backward from the anchor.
    let mut dates: Vec<Option<NaiveDate>> = vec![None; raws.len()];
    let mut last_assigned: Option<(usize, NaiveDate)> = None;
    let mut year = anchor.year();

    // Find the last non-None raw and pin it to the anchor's year (or a best
    // guess if the anchor's month-day differs).
    let mut last_idx = None;
    for i in (0..raws.len()).rev() {
        if raws[i].is_some() {
            last_idx = Some(i);
            break;
        }
    }
    if let Some(i) = last_idx {
        let raw = raws[i].as_ref().unwrap();
        let d = make_date(year, raw.month, raw.day)
            .with_context(|| format!("sheet {:?}: invalid date", raw.sheet_name))?;
        dates[i] = Some(d);
        last_assigned = Some((i, d));
    }

    if let Some((last_i, _)) = last_assigned {
        for i in (0..last_i).rev() {
            let Some(raw) = raws[i].as_ref() else { continue };
            let next_date = dates[(i + 1)..]
                .iter()
                .flatten()
                .next()
                .copied()
                .expect("later date must exist");
            let mut candidate = make_date(year, raw.month, raw.day)
                .with_context(|| format!("sheet {:?}: invalid date", raw.sheet_name))?;
            if candidate >= next_date {
                year -= 1;
                candidate = make_date(year, raw.month, raw.day)
                    .with_context(|| format!("sheet {:?}: invalid date", raw.sheet_name))?;
            }
            dates[i] = Some(candidate);
        }
    }

    // Build the performer registry across all sheets first, so slugs are
    // stable before we reference them from sets.
    let mut perf_by_slug: BTreeMap<Slug, Performer> = BTreeMap::new();
    for raw in raws.iter().flatten() {
        for row in &raw.rows {
            for name in [row.dj.as_deref(), row.vj.as_deref()].into_iter().flatten() {
                register_performer(name, &mut perf_by_slug, &mut plan.merges);
            }
        }
    }
    plan.performers = Performers(perf_by_slug);

    // Build shows.
    let mut shows_by_date: BTreeMap<NaiveDate, PlannedShow> = BTreeMap::new();
    for (i, raw_opt) in raws.iter().enumerate() {
        let Some(raw) = raw_opt else { continue };
        let Some(date) = dates[i] else { continue };

        let mut sets: Vec<Set> = Vec::new();
        // Track the previous effective start in minutes-from-show-midnight,
        // so minute-level comparisons work (22:00 -> 22:15 must not roll over).
        let mut prev_minutes: i32 = -1;
        let mut rollover_min: i32 = 0;
        for row in &raw.rows {
            let Some((h, m)) = row.parsed_time else { continue };
            // Skip rows with neither dj nor vj.
            if row.dj.is_none() && row.vj.is_none() {
                continue;
            }
            let raw_minutes = h as i32 * 60 + m as i32;
            let mut effective_min = raw_minutes + rollover_min;
            // Strict inequality: a row that is exactly equal to the previous
            // is a duplicate (same time / same row in the sheet) - skip it
            // rather than forcing a false rollover.
            if effective_min == prev_minutes {
                continue;
            }
            while effective_min < prev_minutes {
                effective_min += 24 * 60;
                rollover_min += 24 * 60;
            }
            prev_minutes = effective_min;
            if effective_min >= 48 * 60 {
                plan.skipped.push((
                    raw.sheet_name.clone(),
                    format!("time {:02}:{:02} overflowed 47h", h, m),
                ));
                continue;
            }
            sets.push(Set {
                start: WallTime {
                    hour: (effective_min / 60) as u8,
                    minute: (effective_min % 60) as u8,
                },
                duration_min: 60,
                dj: row.dj.as_deref().and_then(slugify),
                vj: row.vj.as_deref().and_then(slugify),
                notes: row.notes.clone(),
            });
        }

        if sets.is_empty() {
            plan.skipped
                .push((raw.sheet_name.clone(), "no valid sets".into()));
            continue;
        }

        let planned = PlannedShow {
            date,
            source_sheet: raw.sheet_name.clone(),
            show: Show { date, sets },
        };

        // Merge by date: if two sheets cover the same date, append sets.
        match shows_by_date.get_mut(&date) {
            Some(existing) => {
                existing.show.sets.extend(planned.show.sets);
                existing.source_sheet.push_str(" + ");
                existing.source_sheet.push_str(&planned.source_sheet);
            }
            None => {
                shows_by_date.insert(date, planned);
            }
        }
    }

    // Re-sort merged sets by start time.
    for planned in shows_by_date.values_mut() {
        planned.show.sets.sort_by_key(|s| (s.start.hour, s.start.minute));
    }

    plan.shows = shows_by_date.into_values().collect();
    Ok(plan)
}

/// Write the plan to disk: `performers.toml` + `shows/YYYY-MM-DD.toml`.
pub fn write_plan(plan: &ImportPlan, data_dir: &Path) -> Result<()> {
    let performers_path = data_dir.join("performers.toml");
    let shows_dir = data_dir.join("shows");
    std::fs::create_dir_all(&shows_dir)?;

    let perf_toml = toml::to_string_pretty(&plan.performers)
        .context("serializing performers")?;
    std::fs::write(&performers_path, perf_toml)
        .with_context(|| format!("writing {}", performers_path.display()))?;

    for planned in &plan.shows {
        let path = shows_dir.join(format!("{}.toml", planned.date));
        let show_toml =
            toml::to_string_pretty(&planned.show).context("serializing show")?;
        std::fs::write(&path, show_toml)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

// --- parsing helpers ---

fn parse_sheet(name: &str, range: &calamine::Range<Data>) -> Result<RawSheet, String> {
    // Locate the DJ header cell anywhere in the sheet. Column layout is
    // derived from its position so we handle sheets whose columns are shifted.
    let mut dj_col: Option<usize> = None;
    let mut header_row: Option<usize> = None;
    'outer: for (r, row) in range.rows().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            if let Some(s) = data_str(cell) {
                if s.trim().eq_ignore_ascii_case("dj") {
                    header_row = Some(r);
                    dj_col = Some(c);
                    break 'outer;
                }
            }
        }
    }
    let header_row = header_row.ok_or_else(|| "no DJ header row".to_string())?;
    let dj_col = dj_col.unwrap();
    if dj_col < 1 {
        return Err("DJ header has no time column to its left".into());
    }
    let time_col = dj_col - 1;
    let vj_col = dj_col + 1;
    let notes_col = dj_col + 2;

    // Find "Day:" cell and read the value to its right.
    let mut day_value: Option<&Data> = None;
    for (r, row) in range.rows().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            if let Some(s) = data_str(cell) {
                if s.trim().eq_ignore_ascii_case("day:") {
                    day_value = range.get((r, c + 1));
                    break;
                }
            }
        }
        if day_value.is_some() {
            break;
        }
    }

    // Fall back from: Day cell -> sheet name -> any string in the first few
    // rows of the sheet (catches sheets like "VKET US" whose title cell has
    // the date while the Day cell is empty).
    let (month, day) = day_value
        .and_then(extract_month_day_from_cell)
        .or_else(|| parse_month_day(name))
        .or_else(|| {
            for (r, row) in range.rows().enumerate() {
                if r > 3 {
                    break;
                }
                for cell in row.iter() {
                    if let Some(s) = data_str(cell) {
                        if let Some(md) = parse_month_day(s) {
                            return Some(md);
                        }
                    }
                }
            }
            None
        })
        .ok_or_else(|| "no month-day in Day cell, sheet name, or title".to_string())?;

    let mut rows: Vec<RawRow> = Vec::new();
    for (r, _) in range.rows().enumerate().skip(header_row + 1) {
        let time_cell = range.get((r, time_col));
        let dj = cell_string(range, r, dj_col).filter(|s| !s.trim().is_empty());
        let vj = cell_string(range, r, vj_col).filter(|s| !s.trim().is_empty());
        let notes = cell_string(range, r, notes_col).unwrap_or_default();
        let parsed_time = time_cell.and_then(parse_time_cell);

        if parsed_time.is_none() && dj.is_none() && vj.is_none() && notes.trim().is_empty() {
            continue;
        }
        rows.push(RawRow {
            parsed_time,
            dj,
            vj,
            notes,
        });
    }

    if rows.is_empty() {
        return Err("no lineup rows".into());
    }

    Ok(RawSheet {
        sheet_name: name.to_string(),
        month,
        day,
        rows,
    })
}

fn cell_string(range: &calamine::Range<Data>, r: usize, c: usize) -> Option<String> {
    range.get((r, c)).and_then(data_str).map(|s| s.to_string())
}

fn data_str(d: &Data) -> Option<&str> {
    match d {
        Data::String(s) => Some(s.as_str()),
        _ => None,
    }
}

/// Parse a time cell, which may be a string like `"21:00ET/10:00JST"` or an
/// Excel datetime whose fractional day is the time of day.
fn parse_time_cell(d: &Data) -> Option<(u8, u8)> {
    match d {
        Data::String(s) => parse_time(s),
        Data::DateTime(dt) => {
            let frac = dt.as_f64().fract().abs();
            let total_min = (frac * 24.0 * 60.0).round() as u32;
            Some(((total_min / 60) as u8, (total_min % 60) as u8))
        }
        Data::Float(f) => {
            let frac = f.fract().abs();
            let total_min = (frac * 24.0 * 60.0).round() as u32;
            Some(((total_min / 60) as u8, (total_min % 60) as u8))
        }
        _ => None,
    }
}

/// Extract month-day from a Day-cell value (string or Excel datetime).
fn extract_month_day_from_cell(d: &Data) -> Option<(u32, u32)> {
    match d {
        Data::String(s) => parse_month_day(s),
        Data::DateTime(dt) => {
            let ndt = dt.as_datetime()?;
            Some((ndt.month(), ndt.day()))
        }
        _ => None,
    }
}

/// Extract the first `M-D` or `M/D` from a string, where M in 1..=12 and D in 1..=31.
fn parse_month_day(s: &str) -> Option<(u32, u32)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            // read up to 2 digits as month
            let mut j = i;
            while j < bytes.len() && j < i + 2 && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b'-' || bytes[j] == b'/') {
                let month: u32 = std::str::from_utf8(&bytes[i..j]).ok()?.parse().ok()?;
                let mut k = j + 1;
                let d_start = k;
                while k < bytes.len() && k < d_start + 2 && bytes[k].is_ascii_digit() {
                    k += 1;
                }
                if k > d_start {
                    let day: u32 = std::str::from_utf8(&bytes[d_start..k]).ok()?.parse().ok()?;
                    if (1..=12).contains(&month) && (1..=31).contains(&day) {
                        return Some((month, day));
                    }
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

/// Parse an ET wall time from the sheet's time cell.
///
/// Accepts shapes like `21:00ET/10:00JST`, `0100ET/16:00JST`, `21:00ET/04:00CET`.
/// Returns (hour, minute), unnormalized (hour may be 0..=24). Caller handles
/// rollover for `24:00` and subsequent lower-hour rows.
fn parse_time(s: &str) -> Option<(u8, u8)> {
    // Locate "ET" case-insensitively, then walk backward to grab the digits.
    let lower = s.to_ascii_lowercase();
    let et_pos = lower.find("et")?;
    let prefix = &s[..et_pos];
    // Grab the trailing digit run (plus optional colon) from prefix.
    let chars: Vec<char> = prefix.chars().collect();
    let mut end = chars.len();
    // strip trailing whitespace
    while end > 0 && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 {
        let c = chars[start - 1];
        if c.is_ascii_digit() || c == ':' {
            start -= 1;
        } else {
            break;
        }
    }
    let time_str: String = chars[start..end].iter().collect();
    let time_str = time_str.trim();
    if time_str.is_empty() {
        return None;
    }
    let (h, m) = if let Some((h, m)) = time_str.split_once(':') {
        (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?)
    } else {
        // Compact form like "0100"
        if time_str.len() < 3 {
            return None;
        }
        let split = time_str.len() - 2;
        let h = time_str[..split].parse::<u32>().ok()?;
        let m = time_str[split..].parse::<u32>().ok()?;
        (h, m)
    };
    if h > 24 || m > 59 {
        return None;
    }
    Some((h as u8, m as u8))
}

fn make_date(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, day)
}

/// Convert a display name into a canonical slug.
///
/// Returns None for empty/unslug-able inputs (caller should skip).
pub fn slugify(name: &str) -> Option<String> {
    let ascii = deunicode(name).to_ascii_lowercase();
    let mut out = String::with_capacity(ascii.len());
    let mut last_underscore = true; // treat start as underscore to avoid leading _
    for c in ascii.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() { None } else { Some(out) }
}

fn register_performer(
    display: &str,
    by_slug: &mut BTreeMap<Slug, Performer>,
    merges: &mut Vec<(Slug, String)>,
) {
    let trimmed = display.trim();
    if trimmed.is_empty() {
        return;
    }
    let Some(slug) = slugify(trimmed) else { return };
    match by_slug.get_mut(&slug) {
        Some(existing) => {
            if existing.display_name != trimmed
                && !existing.aliases.iter().any(|a| a == trimmed)
            {
                existing.aliases.push(trimmed.to_string());
                merges.push((slug, trimmed.to_string()));
            }
        }
        None => {
            by_slug.insert(
                slug,
                Performer {
                    display_name: trimmed.to_string(),
                    aliases: Vec::new(),
                    notes: String::new(),
                    twitch: String::new(),
                    cdn: String::new(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Pizza Princess").as_deref(), Some("pizza_princess"));
        assert_eq!(slugify("S.S.Stripe").as_deref(), Some("s_s_stripe"));
        assert_eq!(slugify("GoblinMode").as_deref(), Some("goblinmode"));
        assert_eq!(slugify("Goblin mode").as_deref(), Some("goblin_mode"));
    }

    #[test]
    fn slugify_unicode() {
        // deunicode romanizes Japanese kana.
        let s = slugify("めぐみ").unwrap();
        assert!(!s.is_empty());
        assert!(s.chars().all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()));
    }

    #[test]
    fn slugify_parens() {
        assert_eq!(
            slugify("Transistors (Morganite/Draco)").as_deref(),
            Some("transistors_morganite_draco")
        );
        assert_eq!(slugify("Hydie (NerdyHydra)").as_deref(), Some("hydie_nerdyhydra"));
    }

    #[test]
    fn parse_time_variants() {
        assert_eq!(parse_time("21:00ET/10:00JST"), Some((21, 0)));
        assert_eq!(parse_time("0100ET/16:00JST"), Some((1, 0)));
        assert_eq!(parse_time("24:00ET/13:00JST"), Some((24, 0)));
        assert_eq!(parse_time("21:00ET/04:00CET"), Some((21, 0)));
        assert_eq!(parse_time("21:30 ET / 10:30 JST"), Some((21, 30)));
    }

    #[test]
    fn parse_time_bad() {
        assert_eq!(parse_time(""), None);
        assert_eq!(parse_time("nope"), None);
    }

    #[test]
    fn parse_month_day_variants() {
        assert_eq!(parse_month_day("3-28(US)/3-29(JP)"), Some((3, 28)));
        assert_eq!(parse_month_day("10-21(US)10-22(JP)"), Some((10, 21)));
        assert_eq!(parse_month_day("12-17/12-18"), Some((12, 17)));
        assert_eq!(parse_month_day("12-24/25"), Some((12, 24)));
        assert_eq!(parse_month_day("Kaleidosky 4-25(US)4-26(JP)"), Some((4, 25)));
        assert_eq!(parse_month_day("KalleidoSky12-1718"), Some((12, 17)));
        assert_eq!(parse_month_day("no dates here"), None);
    }

    #[test]
    fn register_and_merge() {
        let mut by_slug = BTreeMap::new();
        let mut merges = Vec::new();
        register_performer("Goblin mode", &mut by_slug, &mut merges);
        register_performer("Goblin mode", &mut by_slug, &mut merges); // exact dup
        register_performer("goblin_mode", &mut by_slug, &mut merges); // same slug, diff display
        assert_eq!(by_slug.len(), 1);
        let p = by_slug.get("goblin_mode").unwrap();
        assert_eq!(p.display_name, "Goblin mode");
        assert_eq!(p.aliases, vec!["goblin_mode"]);
        assert_eq!(merges.len(), 1);
    }
}
