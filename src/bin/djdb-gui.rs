//! Desktop editor for the KaleidoSky lineup database.
//!
//! Loads the on-disk dataset, lets the user browse/edit shows and
//! performers, then publishes via git add/commit/push. All mutations go
//! through `djdb::commands` or the same serialization helpers the CLI uses,
//! so the UI and CLI stay in lockstep.

use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::NaiveDate;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

use djdb::commands::{self, save_performers, save_show};
use djdb::data::Dataset;
use djdb::performer::{Performer, Performers, is_valid_slug};
use djdb::show::{Set, Show, WallTime};

fn main() -> eframe::Result<()> {
    let data_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(find_data_dir)
        .unwrap_or_else(|| PathBuf::from("data"));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("djdb - KaleidoSky lineup editor"),
        ..Default::default()
    };

    eframe::run_native(
        "djdb",
        options,
        Box::new(move |cc| {
            install_cjk_font(&cc.egui_ctx);
            Ok(Box::new(DjdbApp::new(data_dir)))
        }),
    )
}

/// Register a bundled Japanese font as a fallback so performer names with
/// kana/kanji (e.g. `とかげ／Tokage`) render correctly. The font is embedded
/// at compile time, making the binary self-contained and independent of
/// whatever fonts happen to be installed on the user's machine.
fn install_cjk_font(ctx: &egui::Context) {
    const NOTO_SANS_JP: &[u8] =
        include_bytes!("../../assets/fonts/NotoSansJP-Regular.ttf");
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("jp".into(), egui::FontData::from_static(NOTO_SANS_JP));
    // Append after the default font so Latin text keeps egui's bundled
    // font and CJK characters fall through to Noto.
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push("jp".into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("jp".into());
    ctx.set_fonts(fonts);
}

/// Walk upward from the current working directory and from the binary's
/// location looking for a `data/performers.toml`. This lets users launch
/// the binary from anywhere (file manager, target/release, etc.) without
/// passing an explicit path.
fn find_data_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    for start in candidates {
        let mut dir = start.as_path();
        loop {
            let candidate = dir.join("data").join("performers.toml");
            if candidate.is_file() {
                return Some(dir.join("data"));
            }
            match dir.parent() {
                Some(p) => dir = p,
                None => break,
            }
        }
    }
    None
}

// ============================================================
// app state
// ============================================================

#[derive(Copy, Clone, PartialEq, Eq)]
enum Tab {
    Shows,
    Performers,
    Queries,
}

struct DjdbApp {
    data_dir: PathBuf,
    dataset: Option<Dataset>,
    load_error: Option<String>,
    tab: Tab,

    // Shows tab
    show_filter: String,
    selected_show: Option<NaiveDate>,
    show_draft: Option<ShowDraft>,
    new_show_date: String,

    // Performers tab
    perf_filter: String,
    selected_perf: Option<String>,
    perf_draft: Option<PerformerDraft>,
    new_perf_slug: String,
    new_perf_name: String,

    // Queries tab
    stale_days: i64,
    last_played_input: String,

    // Status
    toast: Option<Toast>,
}

struct Toast {
    text: String,
    error: bool,
}

#[derive(Clone)]
struct ShowDraft {
    date: NaiveDate,
    sets: Vec<SetDraft>,
    dirty: bool,
}

#[derive(Clone, Default)]
struct SetDraft {
    time: String,
    duration: String,
    dj: String,
    vj: String,
    notes: String,
    mark_delete: bool,
}

#[derive(Clone)]
struct PerformerDraft {
    slug: String,
    display_name: String,
    aliases: String, // one per line
    twitch: String,
    cdn: String,
    notes: String,
    dirty: bool,
}

impl DjdbApp {
    fn new(data_dir: PathBuf) -> Self {
        let mut app = Self {
            data_dir,
            dataset: None,
            load_error: None,
            tab: Tab::Shows,
            show_filter: String::new(),
            selected_show: None,
            show_draft: None,
            new_show_date: String::new(),
            perf_filter: String::new(),
            selected_perf: None,
            perf_draft: None,
            new_perf_slug: String::new(),
            new_perf_name: String::new(),
            stale_days: 60,
            last_played_input: String::new(),
            toast: None,
        };
        app.reload();
        app
    }

    fn reload(&mut self) {
        match Dataset::load(&self.data_dir) {
            Ok(ds) => {
                self.dataset = Some(ds);
                self.load_error = None;
            }
            Err(e) => {
                self.dataset = None;
                self.load_error = Some(format!("{e:#}"));
            }
        }
    }

    fn set_ok(&mut self, msg: impl Into<String>) {
        self.toast = Some(Toast {
            text: msg.into(),
            error: false,
        });
    }

    fn set_err(&mut self, msg: impl Into<String>) {
        self.toast = Some(Toast {
            text: msg.into(),
            error: true,
        });
    }
}

// ============================================================
// eframe::App
// ============================================================

impl eframe::App for DjdbApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Shows, "Shows");
                ui.selectable_value(&mut self.tab, Tab::Performers, "Performers");
                ui.selectable_value(&mut self.tab, Tab::Queries, "Queries");
                ui.separator();
                if ui.button("Reload").clicked() {
                    self.reload();
                    self.set_ok("reloaded from disk");
                }
            });
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Publish (git add + commit + push)").clicked() {
                    self.publish();
                }
                ui.separator();
                if let Some(toast) = &self.toast {
                    let color = if toast.error {
                        egui::Color32::LIGHT_RED
                    } else {
                        egui::Color32::LIGHT_GREEN
                    };
                    ui.label(RichText::new(&toast.text).color(color));
                }
            });
        });

        if let Some(err) = self.load_error.clone() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("Failed to load dataset");
                ui.label(err);
                if ui.button("Retry").clicked() {
                    self.reload();
                }
            });
            return;
        }

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Shows => self.ui_shows(ui),
            Tab::Performers => self.ui_performers(ui),
            Tab::Queries => self.ui_queries(ui),
        });
    }
}

// ============================================================
// shows tab
// ============================================================

impl DjdbApp {
    fn ui_shows(&mut self, ui: &mut egui::Ui) {
        // Snapshot list of dates so we don't hold a borrow on self.dataset
        // while building the UI (which needs to call methods on self).
        let mut dates: Vec<NaiveDate> = self
            .dataset
            .as_ref()
            .unwrap()
            .shows
            .iter()
            .map(|s| s.date)
            .collect();
        dates.sort_by(|a, b| b.cmp(a));

        let selected = self.selected_show;
        let mut want_select: Option<NaiveDate> = None;
        let mut do_create = false;

        egui::SidePanel::left("shows_list")
            .resizable(true)
            .default_width(220.0)
            .show_inside(ui, |ui| {
                ui.heading("Shows");
                ui.horizontal(|ui| {
                    ui.label("Filter:");
                    ui.text_edit_singleline(&mut self.show_filter);
                });
                ui.separator();

                let filter = self.show_filter.to_lowercase();
                egui::ScrollArea::vertical()
                    .id_salt("shows_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.with_layout(
                            egui::Layout::top_down(egui::Align::Min)
                                .with_cross_justify(true),
                            |ui| {
                                for date in &dates {
                                    if !filter.is_empty()
                                        && !date.to_string().contains(&filter)
                                    {
                                        continue;
                                    }
                                    if ui
                                        .selectable_label(
                                            selected == Some(*date),
                                            date.to_string(),
                                        )
                                        .clicked()
                                    {
                                        want_select = Some(*date);
                                    }
                                }
                            },
                        );
                    });

                ui.separator();
                ui.label("New show:");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_show_date)
                            .hint_text("YYYY-MM-DD")
                            .desired_width(110.0),
                    );
                    if ui.button("Create").clicked() {
                        do_create = true;
                    }
                });
            });

        if let Some(d) = want_select {
            self.load_show_draft(d);
        }
        if do_create {
            self.create_new_show();
        }

        egui::CentralPanel::default().show_inside(ui, |ui| {
            if self.show_draft.is_none() {
                ui.label("Select a show from the list, or create a new one.");
                return;
            }
            self.ui_show_editor(ui);
        });
    }

    fn load_show_draft(&mut self, date: NaiveDate) {
        let ds = self.dataset.as_ref().unwrap();
        let show = ds.shows.iter().find(|s| s.date == date);
        self.selected_show = Some(date);
        self.show_draft = show.map(|s| ShowDraft {
            date: s.date,
            sets: s.sets.iter().map(SetDraft::from_set).collect(),
            dirty: false,
        });
    }

    fn create_new_show(&mut self) {
        let Ok(date) = NaiveDate::parse_from_str(&self.new_show_date, "%Y-%m-%d") else {
            self.set_err("date must be YYYY-MM-DD");
            return;
        };
        if let Some(ds) = &self.dataset {
            if ds.shows.iter().any(|s| s.date == date) {
                self.set_err(format!("show {date} already exists"));
                return;
            }
        }
        self.selected_show = Some(date);
        self.show_draft = Some(ShowDraft {
            date,
            sets: Vec::new(),
            dirty: true,
        });
        self.new_show_date.clear();
        self.set_ok(format!("drafting new show {date} (not yet saved)"));
    }

    fn ui_show_editor(&mut self, ui: &mut egui::Ui) {
        // Take the draft out and collect a snapshot of read-only dataset
        // info so we can freely mutate self after the UI closures finish.
        let mut draft = self.show_draft.take().unwrap();
        let performers_snapshot: Performers = self.dataset.as_ref().unwrap().performers.clone();
        let disk_snapshot: Option<Show> = self
            .dataset
            .as_ref()
            .unwrap()
            .shows
            .iter()
            .find(|s| s.date == draft.date)
            .cloned();

        ui.horizontal(|ui| {
            ui.heading(draft.date.to_string());
            ui.label(
                RichText::new(draft.date.format("%A").to_string()).color(egui::Color32::GRAY),
            );
            if draft.dirty {
                ui.label(RichText::new("(unsaved)").color(egui::Color32::YELLOW));
            }
        });
        ui.separator();

        // Reserve space for the Save/Revert row + Add-set button below.
        let table_height = (ui.available_height() - 80.0).max(100.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .max_height(table_height)
            .show(ui, |ui| {
                TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::exact(56.0))
                    .column(Column::exact(48.0))
                    .column(Column::initial(180.0).at_least(100.0).resizable(true))
                    .column(Column::initial(180.0).at_least(100.0).resizable(true))
                    .column(Column::remainder().at_least(160.0))
                    .column(Column::exact(28.0))
                    .header(22.0, |mut header| {
                        header.col(|ui| {
                            ui.label(RichText::new("Time").strong());
                        });
                        header.col(|ui| {
                            ui.label(RichText::new("Dur").strong());
                        });
                        header.col(|ui| {
                            ui.label(RichText::new("DJ").strong());
                        });
                        header.col(|ui| {
                            ui.label(RichText::new("VJ").strong());
                        });
                        header.col(|ui| {
                            ui.label(RichText::new("Notes").strong());
                        });
                        header.col(|_| {});
                    })
                    .body(|mut body| {
                        for (i, set) in draft.sets.iter_mut().enumerate() {
                            body.row(26.0, |mut row| {
                                row.col(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut set.time)
                                            .desired_width(f32::INFINITY)
                                            .id(egui::Id::new(("time", i))),
                                    );
                                });
                                row.col(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut set.duration)
                                            .desired_width(f32::INFINITY)
                                            .id(egui::Id::new(("dur", i))),
                                    );
                                });
                                row.col(|ui| {
                                    slug_input(ui, &mut set.dj, &performers_snapshot, ("dj", i));
                                });
                                row.col(|ui| {
                                    slug_input(ui, &mut set.vj, &performers_snapshot, ("vj", i));
                                });
                                row.col(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut set.notes)
                                            .desired_width(f32::INFINITY)
                                            .id(egui::Id::new(("notes", i))),
                                    );
                                });
                                row.col(|ui| {
                                    if ui
                                        .button("×")
                                        .on_hover_text("remove this set")
                                        .clicked()
                                    {
                                        set.mark_delete = true;
                                    }
                                });
                            });
                        }
                    });

                let before = draft.sets.len();
                draft.sets.retain(|s| !s.mark_delete);
                if draft.sets.len() != before {
                    draft.dirty = true;
                }

                ui.add_space(8.0);
                if ui.button("+ Add set").clicked() {
                    draft.sets.push(SetDraft::default_new());
                    draft.dirty = true;
                }
            });

        ui.separator();
        let mut do_save = false;
        let mut do_revert = false;
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                do_save = true;
            }
            if ui.button("Revert").clicked() {
                do_revert = true;
            }
        });

        // Recompute dirty flag from draft vs disk snapshot.
        if !draft.dirty {
            let current = draft_canon(&draft);
            let canon = disk_snapshot.as_ref().map(show_canon);
            if canon.as_ref() != Some(&current) {
                draft.dirty = true;
            }
        }

        self.show_draft = Some(draft);

        // Now self is fully owned again. Perform actions.
        if do_save {
            // Clone draft so we don't hold a borrow while reloading.
            let draft = self.show_draft.as_ref().unwrap().clone();
            match build_show_from_draft(&draft) {
                Ok(show) => match save_show(&self.data_dir, &show) {
                    Ok(()) => {
                        self.set_ok(format!("saved {}", draft.date));
                        if let Some(d) = self.show_draft.as_mut() {
                            d.dirty = false;
                        }
                        self.reload();
                    }
                    Err(e) => self.set_err(format!("save failed: {e:#}")),
                },
                Err(e) => self.set_err(format!("cannot save: {e:#}")),
            }
        }
        if do_revert {
            if let Some(date) = self.selected_show {
                self.load_show_draft(date);
                self.set_ok("reverted");
            }
        }
    }
}

fn draft_canon(draft: &ShowDraft) -> Vec<(String, String, String, String, String)> {
    draft
        .sets
        .iter()
        .map(|s| {
            (
                s.time.clone(),
                s.duration.clone(),
                s.dj.clone(),
                s.vj.clone(),
                s.notes.clone(),
            )
        })
        .collect()
}

fn show_canon(show: &Show) -> Vec<(String, String, String, String, String)> {
    show.sets
        .iter()
        .map(|s| {
            (
                format!("{:02}:{:02}", s.start.hour, s.start.minute),
                s.duration_min.to_string(),
                s.dj.clone().unwrap_or_default(),
                s.vj.clone().unwrap_or_default(),
                s.notes.clone(),
            )
        })
        .collect()
}

impl SetDraft {
    fn from_set(s: &Set) -> Self {
        Self {
            time: format!("{:02}:{:02}", s.start.hour, s.start.minute),
            duration: s.duration_min.to_string(),
            dj: s.dj.clone().unwrap_or_default(),
            vj: s.vj.clone().unwrap_or_default(),
            notes: s.notes.clone(),
            mark_delete: false,
        }
    }

    fn default_new() -> Self {
        Self {
            time: "21:00".into(),
            duration: "60".into(),
            ..Default::default()
        }
    }
}

fn build_show_from_draft(draft: &ShowDraft) -> anyhow::Result<Show> {
    if draft.sets.is_empty() {
        anyhow::bail!("show needs at least one set");
    }
    let mut sets = Vec::with_capacity(draft.sets.len());
    for (i, s) in draft.sets.iter().enumerate() {
        let start = WallTime::parse(s.time.trim())
            .map_err(|e| anyhow::anyhow!("row {}: bad time {:?}: {e}", i + 1, s.time))?;
        let duration_min: u32 = s
            .duration
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("row {}: duration must be a number", i + 1))?;
        let dj = non_empty(&s.dj);
        let vj = non_empty(&s.vj);
        if dj.is_none() && vj.is_none() {
            anyhow::bail!("row {}: needs a DJ or VJ slug", i + 1);
        }
        sets.push(Set {
            start,
            duration_min,
            dj,
            vj,
            notes: s.notes.clone(),
        });
    }
    sets.sort_by_key(|s| (s.start.hour, s.start.minute));
    Ok(Show {
        date: draft.date,
        sets,
    })
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}

/// Text input for a slug with a colored edge indicating whether it resolves.
/// Uses all available width (intended for use inside a TableBuilder cell).
fn slug_input(
    ui: &mut egui::Ui,
    value: &mut String,
    performers: &Performers,
    id: impl std::hash::Hash,
) {
    let known = value.trim().is_empty() || performers.contains(value.trim());
    let color = if value.trim().is_empty() {
        egui::Color32::GRAY
    } else if known {
        egui::Color32::LIGHT_GREEN
    } else {
        egui::Color32::LIGHT_RED
    };
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(f32::INFINITY)
            .id(egui::Id::new(id))
            .text_color(color),
    );
}

// ============================================================
// performers tab
// ============================================================

impl DjdbApp {
    fn ui_performers(&mut self, ui: &mut egui::Ui) {
        // Snapshot (slug, display_name) so the sidebar closure doesn't hold
        // a dataset borrow while we dispatch actions on self.
        let entries: Vec<(String, String, String)> = self
            .dataset
            .as_ref()
            .unwrap()
            .performers
            .0
            .iter()
            .map(|(slug, p)| (slug.clone(), p.display_name.clone(), p.aliases.join(" ")))
            .collect();

        let selected_slug = self.selected_perf.clone();
        let mut want_select: Option<String> = None;
        let mut do_create = false;

        egui::SidePanel::left("perf_list")
            .resizable(true)
            .default_width(240.0)
            .show_inside(ui, |ui| {
                ui.heading("Performers");
                ui.horizontal(|ui| {
                    ui.label("Filter:");
                    ui.text_edit_singleline(&mut self.perf_filter);
                });
                ui.separator();

                let filter = self.perf_filter.to_lowercase();
                egui::ScrollArea::vertical()
                    .id_salt("perf_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.with_layout(
                            egui::Layout::top_down(egui::Align::Min)
                                .with_cross_justify(true),
                            |ui| {
                                for (slug, display, aliases) in &entries {
                                    if !filter.is_empty() {
                                        let hay = format!(
                                            "{} {} {}",
                                            slug,
                                            display.to_lowercase(),
                                            aliases.to_lowercase()
                                        );
                                        if !hay.contains(&filter) {
                                            continue;
                                        }
                                    }
                                    let is_selected =
                                        selected_slug.as_deref() == Some(slug.as_str());
                                    if ui.selectable_label(is_selected, display).clicked() {
                                        want_select = Some(slug.clone());
                                    }
                                }
                            },
                        );
                    });

                ui.separator();
                ui.label("New performer:");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_perf_slug)
                            .hint_text("slug")
                            .desired_width(90.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_perf_name)
                            .hint_text("Display Name")
                            .desired_width(140.0),
                    );
                    if ui.button("Create").clicked() {
                        do_create = true;
                    }
                });
            });

        if let Some(s) = want_select {
            self.load_perf_draft(&s);
        }
        if do_create {
            self.create_new_performer();
        }

        egui::CentralPanel::default().show_inside(ui, |ui| {
            if self.perf_draft.is_none() {
                ui.label("Select a performer from the list, or create a new one.");
                return;
            }
            self.ui_perf_editor(ui);
        });
    }

    fn load_perf_draft(&mut self, slug: &str) {
        let ds = self.dataset.as_ref().unwrap();
        if let Some(p) = ds.performers.get(slug) {
            self.selected_perf = Some(slug.to_string());
            self.perf_draft = Some(PerformerDraft {
                slug: slug.to_string(),
                display_name: p.display_name.clone(),
                aliases: p.aliases.join("\n"),
                twitch: p.twitch.clone(),
                cdn: p.cdn.clone(),
                notes: p.notes.clone(),
                dirty: false,
            });
        }
    }

    fn create_new_performer(&mut self) {
        let slug = self.new_perf_slug.trim().to_string();
        let name = self.new_perf_name.trim().to_string();
        if !is_valid_slug(&slug) {
            self.set_err("slug must be a-z, 0-9, underscore");
            return;
        }
        if name.is_empty() {
            self.set_err("display name required");
            return;
        }
        match commands::new_performer(
            &self.data_dir,
            commands::NewPerformerArgs {
                slug: &slug,
                display_name: &name,
                twitch: "",
                cdn: "",
                notes: "",
                aliases: vec![],
            },
        ) {
            Ok(()) => {
                self.set_ok(format!("created {slug}"));
                self.new_perf_slug.clear();
                self.new_perf_name.clear();
                self.reload();
                self.load_perf_draft(&slug);
            }
            Err(e) => self.set_err(format!("{e:#}")),
        }
    }

    fn ui_perf_editor(&mut self, ui: &mut egui::Ui) {
        let mut draft = self.perf_draft.take().unwrap();

        // Precompute appearance history from the borrowed dataset.
        let appearances: Vec<(NaiveDate, &'static str, String)> = {
            let ds = self.dataset.as_ref().unwrap();
            let mut out: Vec<(NaiveDate, &'static str, String)> = Vec::new();
            for show in &ds.shows {
                for set in &show.sets {
                    let time = format!("{:02}:{:02}", set.start.hour, set.start.minute);
                    if set.dj.as_deref() == Some(&draft.slug) {
                        out.push((show.date, "DJ", time.clone()));
                    }
                    if set.vj.as_deref() == Some(&draft.slug) {
                        out.push((show.date, "VJ", time));
                    }
                }
            }
            out.sort_by(|a, b| b.0.cmp(&a.0));
            out
        };

        ui.horizontal(|ui| {
            ui.heading(&draft.display_name);
            ui.label(RichText::new(&draft.slug).color(egui::Color32::GRAY));
            if draft.dirty {
                ui.label(RichText::new("(unsaved)").color(egui::Color32::YELLOW));
            }
        });
        ui.separator();

        egui::Grid::new("perf_form")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Display name:");
                if ui.text_edit_singleline(&mut draft.display_name).changed() {
                    draft.dirty = true;
                }
                ui.end_row();

                ui.label("Twitch:");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut draft.twitch)
                            .desired_width(400.0)
                            .hint_text("https://twitch.tv/..."),
                    )
                    .changed()
                {
                    draft.dirty = true;
                }
                ui.end_row();

                ui.label("CDN:");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut draft.cdn)
                            .desired_width(400.0)
                            .hint_text("rtspt://..."),
                    )
                    .changed()
                {
                    draft.dirty = true;
                }
                ui.end_row();

                ui.label("Aliases:");
                if ui
                    .add(
                        egui::TextEdit::multiline(&mut draft.aliases)
                            .desired_rows(2)
                            .desired_width(400.0)
                            .hint_text("one per line"),
                    )
                    .changed()
                {
                    draft.dirty = true;
                }
                ui.end_row();

                ui.label("Notes:");
                if ui
                    .add(
                        egui::TextEdit::multiline(&mut draft.notes)
                            .desired_rows(4)
                            .desired_width(400.0),
                    )
                    .changed()
                {
                    draft.dirty = true;
                }
                ui.end_row();
            });

        ui.separator();
        let mut do_save = false;
        let mut do_revert = false;
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                do_save = true;
            }
            if ui.button("Revert").clicked() {
                do_revert = true;
            }
        });

        ui.separator();
        ui.heading("Appearances");
        ui.label(format!("{} total", appearances.len()));
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                egui::Grid::new("hist_grid")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        for (d, role, time) in appearances {
                            ui.label(d.to_string());
                            ui.label(role);
                            ui.label(time);
                            ui.end_row();
                        }
                    });
            });

        self.perf_draft = Some(draft);

        if do_save {
            let draft = self.perf_draft.as_ref().unwrap().clone();
            match self.save_perf_draft(&draft) {
                Ok(()) => {
                    self.set_ok(format!("saved {}", draft.slug));
                    if let Some(d) = self.perf_draft.as_mut() {
                        d.dirty = false;
                    }
                    self.reload();
                }
                Err(e) => self.set_err(format!("{e:#}")),
            }
        }
        if do_revert {
            let slug = self.perf_draft.as_ref().unwrap().slug.clone();
            self.load_perf_draft(&slug);
            self.set_ok("reverted");
        }
    }

    fn save_perf_draft(&self, draft: &PerformerDraft) -> anyhow::Result<()> {
        let mut performers = Performers::load(&self.data_dir.join("performers.toml"))?;
        let p = performers
            .0
            .get_mut(&draft.slug)
            .ok_or_else(|| anyhow::anyhow!("performer {:?} not found", draft.slug))?;
        if draft.display_name.trim().is_empty() {
            anyhow::bail!("display name required");
        }
        p.display_name = draft.display_name.trim().to_string();
        p.twitch = draft.twitch.trim().to_string();
        p.cdn = draft.cdn.trim().to_string();
        p.notes = draft.notes.clone();
        p.aliases = draft
            .aliases
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        save_performers(&self.data_dir, &performers)?;
        Ok(())
    }
}

// ============================================================
// queries tab
// ============================================================

impl DjdbApp {
    fn ui_queries(&mut self, ui: &mut egui::Ui) {
        let ds = self.dataset.as_ref().unwrap();
        let today = chrono::Local::now().date_naive();

        ui.heading("Queries");

        ui.collapsing("Last played", |ui| {
            ui.horizontal(|ui| {
                ui.label("Search:");
                ui.text_edit_singleline(&mut self.last_played_input);
            });
            let query = self.last_played_input.trim().to_lowercase();
            if !query.is_empty() {
                let last_map = commands::last_played_map(ds);
                let mut matches: Vec<(&String, &Performer, Option<NaiveDate>)> = ds
                    .performers
                    .0
                    .iter()
                    .filter(|(slug, p)| {
                        slug.to_lowercase().contains(&query)
                            || p.display_name.to_lowercase().contains(&query)
                            || p
                                .aliases
                                .iter()
                                .any(|a| a.to_lowercase().contains(&query))
                    })
                    .map(|(slug, p)| (slug, p, last_map.get(slug).copied()))
                    .collect();
                // Most recently played first, never-played last.
                matches.sort_by(|a, b| match (a.2, b.2) {
                    (Some(ad), Some(bd)) => bd.cmp(&ad),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a.0.cmp(b.0),
                });

                ui.label(format!("{} match(es)", matches.len()));
                egui::ScrollArea::vertical()
                    .id_salt("lp_scroll")
                    .max_height(260.0)
                    .show(ui, |ui| {
                        egui::Grid::new("lp_grid")
                            .num_columns(3)
                            .striped(true)
                            .show(ui, |ui| {
                                for (slug, p, last) in matches {
                                    ui.label(&p.display_name);
                                    ui.label(RichText::new(slug).color(egui::Color32::GRAY));
                                    match last {
                                        Some(d) => {
                                            let ago = (today - d).num_days();
                                            ui.label(format!("{d} ({ago}d ago)"));
                                        }
                                        None => {
                                            ui.label("never");
                                        }
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            }
        });

        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Stale threshold (days):");
            ui.add(egui::DragValue::new(&mut self.stale_days).range(1..=3650));
        });
        let cutoff = today - chrono::Duration::days(self.stale_days);
        let last_map = commands::last_played_map(ds);
        let mut rows: Vec<(String, Option<NaiveDate>)> = Vec::new();
        for slug in ds.performers.0.keys() {
            match last_map.get(slug) {
                Some(&d) if d < cutoff => rows.push((slug.clone(), Some(d))),
                None => rows.push((slug.clone(), None)),
                _ => {}
            }
        }
        rows.sort_by_key(|(_, d)| match d {
            None => (0i64, String::new()),
            Some(d) => (1, d.to_string()),
        });

        ui.label(format!("{} stale performers", rows.len()));
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                egui::Grid::new("stale_grid")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.label(RichText::new("Slug").strong());
                        ui.label(RichText::new("Last played").strong());
                        ui.label(RichText::new("Days ago").strong());
                        ui.end_row();
                        for (slug, last) in rows {
                            ui.label(&slug);
                            match last {
                                Some(d) => {
                                    ui.label(d.to_string());
                                    ui.label((today - d).num_days().to_string());
                                }
                                None => {
                                    ui.label("never");
                                    ui.label("");
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
    }
}

// ============================================================
// publish (git add + commit + push)
// ============================================================

impl DjdbApp {
    fn publish(&mut self) {
        match run_publish(&self.data_dir) {
            Ok(msg) => self.set_ok(msg),
            Err(e) => self.set_err(format!("{e:#}")),
        }
    }
}

fn run_publish(data_dir: &Path) -> anyhow::Result<String> {
    // Repo root = parent of data dir, unless data_dir is itself the root.
    let repo_root = data_dir
        .canonicalize()
        .ok()
        .and_then(|p| {
            if p.join(".git").exists() {
                Some(p)
            } else {
                p.parent().map(|p| p.to_path_buf())
            }
        })
        .unwrap_or_else(|| PathBuf::from("."));

    let status = Command::new("git")
        .arg("-C")
        .arg(&repo_root)
        .args(["status", "--porcelain", "--", "data"])
        .output()?;
    if !status.status.success() {
        anyhow::bail!(
            "git status failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }
    if status.stdout.is_empty() {
        return Ok("nothing to publish".into());
    }

    run_git(&repo_root, &["add", "--", "data"])?;
    run_git(
        &repo_root,
        &["commit", "-m", "Update lineup data from djdb-gui"],
    )?;
    run_git(&repo_root, &["push"])?;
    Ok("published".into())
}

fn run_git(cwd: &Path, args: &[&str]) -> anyhow::Result<()> {
    let out = Command::new("git").arg("-C").arg(cwd).args(args).output()?;
    if !out.status.success() {
        anyhow::bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}
