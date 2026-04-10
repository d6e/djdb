//! Static site generator. Produces `docs/` from the loaded dataset.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, LocalResult, NaiveDate, TimeZone};
use chrono_tz::America::New_York;
use chrono_tz::Asia::Tokyo;
use chrono_tz::Tz;
use maud::{DOCTYPE, Markup, html};

use crate::data::Dataset;
use crate::performer::{Performer, Performers};
use crate::show::{Show, WallTime};

pub struct BuildConfig {
    /// URL prefix for all site links. `"/"` for root deploys, `"/djdb/"` for
    /// GitHub Pages project sites.
    pub base_url: String,
    /// Today's date, used to pick the "next upcoming show" on the landing.
    pub today: NaiveDate,
}

pub fn build(ds: &Dataset, cfg: &BuildConfig, out_dir: &Path) -> Result<()> {
    if out_dir.exists() {
        // Only wipe subdirs/files we own to avoid clobbering unrelated files.
        for entry in std::fs::read_dir(out_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(name.as_ref(), "index.html" | "style.css" | "shows" | "performers") {
                let path = entry.path();
                if path.is_dir() {
                    std::fs::remove_dir_all(&path)?;
                } else {
                    std::fs::remove_file(&path)?;
                }
            }
        }
    }
    std::fs::create_dir_all(out_dir)?;

    // Landing.
    write_page(out_dir, "index.html", &landing(ds, cfg))?;

    // Shows archive.
    let shows_dir = out_dir.join("shows");
    std::fs::create_dir_all(&shows_dir)?;
    write_page_at(shows_dir.join("index.html"), &shows_index(ds, cfg))?;
    for show in &ds.shows {
        let dir = shows_dir.join(show.date.to_string());
        std::fs::create_dir_all(&dir)?;
        write_page_at(dir.join("index.html"), &show_page(show, ds, cfg))?;
    }

    // Performers index + per-performer pages.
    let performers_dir = out_dir.join("performers");
    std::fs::create_dir_all(&performers_dir)?;
    let appearances = index_appearances(ds);
    write_page_at(
        performers_dir.join("index.html"),
        &performers_index(ds, &appearances, cfg),
    )?;
    for (slug, performer) in &ds.performers.0 {
        let dir = performers_dir.join(slug);
        std::fs::create_dir_all(&dir)?;
        let empty = Vec::new();
        let history = appearances.get(slug).unwrap_or(&empty);
        write_page_at(
            dir.join("index.html"),
            &performer_page(slug, performer, history, cfg),
        )?;
    }

    // Stylesheet.
    std::fs::write(out_dir.join("style.css"), STYLE_CSS)?;
    Ok(())
}

fn write_page(out_dir: &Path, rel: &str, markup: &Markup) -> Result<()> {
    write_page_at(out_dir.join(rel), markup)
}

fn write_page_at(path: PathBuf, markup: &Markup) -> Result<()> {
    std::fs::write(&path, markup.clone().into_string())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

// ---------- URL helper ----------

fn url(cfg: &BuildConfig, path: &str) -> String {
    let base = cfg.base_url.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        format!("{base}/")
    } else {
        format!("{base}/{path}")
    }
}

// ---------- ET -> JST conversion ----------

/// Convert a show date + wall-clock ET start time into an absolute
/// `America/New_York` datetime. Handles `WallTime` hours 0..47 by rolling
/// the date forward, and correctly navigates DST transitions.
pub fn to_et_datetime(date: NaiveDate, wall: WallTime) -> DateTime<Tz> {
    let day_offset = (wall.hour / 24) as i64;
    let hour = (wall.hour % 24) as u32;
    let date = date + Duration::days(day_offset);
    let naive = date
        .and_hms_opt(hour, wall.minute as u32, 0)
        .expect("valid h/m");
    match New_York.from_local_datetime(&naive) {
        LocalResult::Single(dt) => dt,
        // Fall-back: pick the earlier (pre-transition) instant, which is what
        // most people mean when they write `01:30 ET` on a fall-back night.
        LocalResult::Ambiguous(earlier, _) => earlier,
        // Spring-forward: the wall time doesn't exist. Walk forward until
        // we land on a valid instant (effectively "shift to the post-jump
        // equivalent").
        LocalResult::None => {
            let bumped = naive + Duration::hours(1);
            New_York
                .from_local_datetime(&bumped)
                .earliest()
                .expect("post-spring-forward must exist")
        }
    }
}

fn format_et_jst(date: NaiveDate, wall: WallTime) -> (String, String) {
    let et = to_et_datetime(date, wall);
    let jst = et.with_timezone(&Tokyo);
    (
        et.format("%H:%M").to_string(),
        jst.format("%H:%M").to_string(),
    )
}

// ---------- appearances index ----------

pub struct Appearance {
    pub date: NaiveDate,
    pub role: &'static str, // "DJ" or "VJ"
    pub start: WallTime,
    pub notes: String,
}

fn index_appearances(ds: &Dataset) -> BTreeMap<String, Vec<Appearance>> {
    let mut out: BTreeMap<String, Vec<Appearance>> = BTreeMap::new();
    for show in &ds.shows {
        for set in &show.sets {
            if let Some(slug) = &set.dj {
                out.entry(slug.clone()).or_default().push(Appearance {
                    date: show.date,
                    role: "DJ",
                    start: set.start,
                    notes: set.notes.clone(),
                });
            }
            if let Some(slug) = &set.vj {
                out.entry(slug.clone()).or_default().push(Appearance {
                    date: show.date,
                    role: "VJ",
                    start: set.start,
                    notes: set.notes.clone(),
                });
            }
        }
    }
    // Newest first.
    for v in out.values_mut() {
        v.sort_by(|a, b| b.date.cmp(&a.date).then(a.start.hour.cmp(&b.start.hour)));
    }
    out
}

// ---------- layout ----------

fn layout(cfg: &BuildConfig, page_title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (page_title) " · KaleidoSky" }
                link rel="stylesheet" href=(url(cfg, "style.css"));
            }
            body {
                header {
                    nav {
                        a href=(url(cfg, "")) class="site-title" { "KaleidoSky" }
                        " · "
                        a href=(url(cfg, "shows/")) { "Shows" }
                        " · "
                        a href=(url(cfg, "performers/")) { "Performers" }
                    }
                }
                main { (body) }
                footer { "generated with djdb" }
            }
        }
    }
}

// ---------- components ----------

fn lineup_table(show: &Show, performers: &Performers, cfg: &BuildConfig) -> Markup {
    html! {
        table class="lineup" {
            thead {
                tr {
                    th { "ET" }
                    th { "JST" }
                    th { "DJ" }
                    th { "VJ" }
                    th { "Notes" }
                }
            }
            tbody {
                @for set in &show.sets {
                    @let (et, jst) = format_et_jst(show.date, set.start);
                    tr {
                        td class="time" { (et) }
                        td class="time" { (jst) }
                        td { (performer_link(set.dj.as_deref(), performers, cfg)) }
                        td { (performer_link(set.vj.as_deref(), performers, cfg)) }
                        td class="notes" { (set.notes) }
                    }
                }
            }
        }
    }
}

fn performer_link(
    slug: Option<&str>,
    performers: &Performers,
    cfg: &BuildConfig,
) -> Markup {
    let Some(slug) = slug else {
        return html! {};
    };
    let Some(p) = performers.get(slug) else {
        return html! { (slug) };
    };
    html! {
        a href=(url(cfg, &format!("performers/{slug}/"))) { (p.display_name) }
    }
}

// ---------- page renderers ----------

fn landing(ds: &Dataset, cfg: &BuildConfig) -> Markup {
    let upcoming = ds.shows.iter().find(|s| s.date >= cfg.today);
    let recent: Vec<&Show> = ds
        .shows
        .iter()
        .rev()
        .filter(|s| s.date < cfg.today)
        .take(8)
        .collect();

    layout(
        cfg,
        "KaleidoSky",
        html! {
            h1 { "KaleidoSky" }
            p class="tagline" { "Weekly DJ lineups in VRChat." }

            @if let Some(show) = upcoming {
                section class="next-show" {
                    h2 {
                        "Next show · "
                        (show.date.format("%A, %B %-e, %Y").to_string())
                    }
                    (lineup_table(show, &ds.performers, cfg))
                }
            } @else {
                p { "No upcoming shows scheduled." }
            }

            @if !recent.is_empty() {
                section class="recent" {
                    h2 { "Recent shows" }
                    ul class="show-list" {
                        @for s in recent {
                            li {
                                a href=(url(cfg, &format!("shows/{}/", s.date))) {
                                    (s.date.format("%Y-%m-%d").to_string())
                                }
                                " · " (s.sets.len()) " sets"
                            }
                        }
                    }
                }
            }
        },
    )
}

fn shows_index(ds: &Dataset, cfg: &BuildConfig) -> Markup {
    layout(
        cfg,
        "Shows",
        html! {
            h1 { "Shows" }
            p { (ds.shows.len()) " shows in the archive." }
            ul class="show-list" {
                @for s in ds.shows.iter().rev() {
                    li {
                        a href=(url(cfg, &format!("shows/{}/", s.date))) {
                            (s.date.format("%Y-%m-%d · %a").to_string())
                        }
                        " · " (s.sets.len()) " sets"
                    }
                }
            }
        },
    )
}

fn show_page(show: &Show, ds: &Dataset, cfg: &BuildConfig) -> Markup {
    layout(
        cfg,
        &show.date.to_string(),
        html! {
            h1 { (show.date.format("%A, %B %-e, %Y").to_string()) }
            p class="subtitle" {
                "KaleidoSky lineup"
            }
            (lineup_table(show, &ds.performers, cfg))
            p {
                a href=(url(cfg, "shows/")) { "← all shows" }
            }
        },
    )
}

fn performers_index(
    ds: &Dataset,
    appearances: &BTreeMap<String, Vec<Appearance>>,
    cfg: &BuildConfig,
) -> Markup {
    layout(
        cfg,
        "Performers",
        html! {
            h1 { "Performers" }
            p { (ds.performers.0.len()) " performers." }
            ul class="performer-list" {
                @for (slug, p) in &ds.performers.0 {
                    @let count = appearances.get(slug).map(|v| v.len()).unwrap_or(0);
                    li {
                        a href=(url(cfg, &format!("performers/{slug}/"))) {
                            (p.display_name)
                        }
                        " · " (count) " sets"
                    }
                }
            }
        },
    )
}

fn performer_page(
    slug: &str,
    performer: &Performer,
    history: &[Appearance],
    cfg: &BuildConfig,
) -> Markup {
    layout(
        cfg,
        &performer.display_name,
        html! {
            h1 { (performer.display_name) }
            p class="slug-line" { "slug: " code { (slug) } }
            @if !performer.aliases.is_empty() {
                p { "Also known as: "
                    @for (i, a) in performer.aliases.iter().enumerate() {
                        @if i > 0 { ", " }
                        (a)
                    }
                }
            }
            @if !performer.twitch.is_empty() {
                p { "Twitch: " a href=(&performer.twitch) { (&performer.twitch) } }
            }
            @if !performer.cdn.is_empty() {
                p { "CDN: " code { (&performer.cdn) } }
            }
            @if !performer.notes.is_empty() {
                section class="notes" {
                    h2 { "Notes" }
                    pre { (performer.notes) }
                }
            }

            h2 { "Appearances (" (history.len()) ")" }
            @if history.is_empty() {
                p { "No recorded appearances." }
            } @else {
                table class="history" {
                    thead {
                        tr { th { "Date" } th { "Role" } th { "ET" } th { "JST" } th { "Notes" } }
                    }
                    tbody {
                        @for a in history {
                            @let (et, jst) = format_et_jst(a.date, a.start);
                            tr {
                                td {
                                    a href=(url(cfg, &format!("shows/{}/", a.date))) {
                                        (a.date.to_string())
                                    }
                                }
                                td { (a.role) }
                                td class="time" { (et) }
                                td class="time" { (jst) }
                                td class="notes" { (a.notes) }
                            }
                        }
                    }
                }
            }
        },
    )
}

// ---------- stylesheet ----------

const STYLE_CSS: &str = r#"
:root {
    --bg: #0b1e2a;
    --fg: #e8f0f4;
    --dim: #8fa2ab;
    --accent: #f0c987;
    --panel: #12263a;
    --rule: #1e3a52;
}
* { box-sizing: border-box; }
html, body {
    margin: 0;
    padding: 0;
    background: var(--bg);
    color: var(--fg);
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
    line-height: 1.5;
}
body { max-width: 960px; margin: 0 auto; padding: 1rem; }
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
header { border-bottom: 1px solid var(--rule); padding-bottom: 1rem; margin-bottom: 2rem; }
header nav { font-size: 1rem; color: var(--dim); }
header nav .site-title { font-weight: bold; color: var(--fg); font-size: 1.1rem; }
footer { margin-top: 4rem; padding-top: 1rem; border-top: 1px solid var(--rule); color: var(--dim); font-size: 0.85rem; }
h1 { margin: 0.5rem 0 1rem; }
h2 { margin-top: 2rem; border-bottom: 1px solid var(--rule); padding-bottom: 0.3rem; }
.tagline { color: var(--dim); margin-top: -0.5rem; }
.subtitle { color: var(--dim); margin-top: -0.5rem; }
.slug-line { color: var(--dim); font-size: 0.9rem; }
code { background: var(--panel); padding: 0.1em 0.4em; border-radius: 3px; font-size: 0.9em; }
pre { background: var(--panel); padding: 0.8rem; border-radius: 4px; overflow-x: auto; }
table { border-collapse: collapse; width: 100%; margin: 1rem 0; }
table th, table td { padding: 0.5rem 0.6rem; text-align: left; border-bottom: 1px solid var(--rule); vertical-align: top; }
table th { color: var(--dim); font-weight: normal; font-size: 0.85rem; text-transform: uppercase; letter-spacing: 0.05em; }
td.time { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; color: var(--dim); white-space: nowrap; }
td.notes { color: var(--dim); font-size: 0.9rem; }
.lineup tbody tr:hover { background: var(--panel); }
.show-list, .performer-list { list-style: none; padding: 0; }
.show-list li, .performer-list li { padding: 0.4rem 0; border-bottom: 1px solid var(--rule); }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn et_standard_time_to_jst() {
        // 2025-01-15 21:00 ET is EST (UTC-5) -> 02:00 UTC -> 11:00 JST next day.
        let date = NaiveDate::from_ymd_opt(2025, 1, 15).unwrap();
        let (et, jst) = format_et_jst(date, WallTime { hour: 21, minute: 0 });
        assert_eq!(et, "21:00");
        assert_eq!(jst, "11:00");
    }

    #[test]
    fn et_daylight_time_to_jst() {
        // 2025-07-15 21:00 ET is EDT (UTC-4) -> 01:00 UTC -> 10:00 JST next day.
        let date = NaiveDate::from_ymd_opt(2025, 7, 15).unwrap();
        let (_et, jst) = format_et_jst(date, WallTime { hour: 21, minute: 0 });
        assert_eq!(jst, "10:00");
    }

    #[test]
    fn overnight_rollover_crosses_dst_spring_forward() {
        // 2025-03-08 show, set at 26:00 (= 2am on 2025-03-09) lands inside
        // the spring-forward gap (2->3am). to_et_datetime must bump to the
        // next valid instant rather than panic or produce garbage.
        let date = NaiveDate::from_ymd_opt(2025, 3, 8).unwrap();
        let dt = to_et_datetime(date, WallTime { hour: 26, minute: 0 });
        // Resulting datetime should be 2025-03-09 03:00 EDT.
        assert_eq!(dt.date_naive(), NaiveDate::from_ymd_opt(2025, 3, 9).unwrap());
        assert_eq!(dt.hour(), 3);
    }

    #[test]
    fn overnight_rollover_across_fall_back() {
        // 2025-11-01 show, set at 25:30 (= 1:30am on 2025-11-02) is
        // ambiguous (occurs twice during fall-back). Pick the earlier.
        let date = NaiveDate::from_ymd_opt(2025, 11, 1).unwrap();
        let dt = to_et_datetime(date, WallTime { hour: 25, minute: 30 });
        // Earlier instance is still EDT (UTC-4), so 01:30 EDT = 05:30 UTC.
        assert_eq!(dt.to_utc().hour(), 5);
        assert_eq!(dt.to_utc().minute(), 30);
    }

    #[test]
    fn url_helper() {
        let cfg = BuildConfig {
            base_url: "/".into(),
            today: NaiveDate::from_ymd_opt(2026, 5, 9).unwrap(),
        };
        assert_eq!(url(&cfg, "shows/"), "/shows/");
        assert_eq!(url(&cfg, ""), "/");

        let cfg = BuildConfig {
            base_url: "/djdb/".into(),
            today: NaiveDate::from_ymd_opt(2026, 5, 9).unwrap(),
        };
        assert_eq!(url(&cfg, "shows/"), "/djdb/shows/");
        assert_eq!(url(&cfg, ""), "/djdb/");
    }

    #[test]
    fn build_produces_all_pages() {
        use crate::performer::{Performer, Performers};
        use crate::show::Set;
        use std::collections::BTreeMap;
        use tempfile::tempdir;

        let mut performers = BTreeMap::new();
        performers.insert(
            "aliquem".to_string(),
            Performer {
                display_name: "Aliquem".into(),
                aliases: vec![],
                notes: String::new(),
                twitch: String::new(),
                cdn: String::new(),
            },
        );
        let ds = Dataset {
            performers: Performers(performers),
            shows: vec![Show {
                date: NaiveDate::from_ymd_opt(2025, 3, 28).unwrap(),
                sets: vec![Set {
                    start: WallTime { hour: 20, minute: 0 },
                    duration_min: 60,
                    dj: Some("aliquem".into()),
                    vj: None,
                    notes: "takeover".into(),
                }],
            }],
        };
        let cfg = BuildConfig {
            base_url: "/".into(),
            today: NaiveDate::from_ymd_opt(2026, 5, 9).unwrap(),
        };
        let dir = tempdir().unwrap();
        build(&ds, &cfg, dir.path()).unwrap();

        assert!(dir.path().join("index.html").exists());
        assert!(dir.path().join("style.css").exists());
        assert!(dir.path().join("shows/index.html").exists());
        assert!(dir.path().join("shows/2025-03-28/index.html").exists());
        assert!(dir.path().join("performers/index.html").exists());
        assert!(dir.path().join("performers/aliquem/index.html").exists());

        let show_html =
            std::fs::read_to_string(dir.path().join("shows/2025-03-28/index.html")).unwrap();
        assert!(show_html.contains("Aliquem"));
        assert!(show_html.contains("takeover"));
    }

}
