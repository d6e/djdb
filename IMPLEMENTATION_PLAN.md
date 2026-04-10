# djdb implementation plan

Static-site + CLI for managing KaleidoSky DJ lineups. Data lives in TOML in the
repo, a Rust binary validates / queries / generates a static site, GitHub Pages
serves it, GitHub Actions builds on push.

## Repo layout

```
data/performers.toml         # all performers, keyed by slug
data/shows/YYYY-MM-DD.toml   # one file per show
src/                         # Rust CLI + generator
templates/                   # HTML templates
static/                      # CSS / assets
docs/                        # generated output (GitHub Pages source)
```

## Stages

### Stage 1: schema, parser, validator, `djdb check`

Goal: load performers + all shows from disk, cross-validate, report errors.

Success criteria:
- `cargo run -- check` exits 0 on valid data, nonzero on any error.
- Errors include: malformed TOML, unknown performer slug referenced in a set,
  bad `HH:MM` time, bad date, duplicate slug, empty lineup.
- Unit tests cover each error class and a happy-path fixture.

Tests:
- Fixture dataset under `tests/fixtures/valid/` loads clean.
- Fixtures under `tests/fixtures/invalid/*` each surface their expected error.

Status: done. 14 unit tests covering slug validation, time parsing, show
shape validation, filename/date mismatch, unknown performer references, and
happy-path load.

### Stage 2: mutation + query CLI

Commands: `add-set` (creates the show file on first use), `new-performer`,
`rename-performer`, `merge-performer`, `last-played`, `stale`.

Success criteria:
- Each command round-trips through `check` without errors.
- `rename-performer` updates `performers.toml` and every referencing show.
- Tests for each command against a tempdir fixture.

Status: done. 13 command tests, smoke-tested against the real imported
dataset (`last-played aliquem` = 2026-05-02, `stale --days 90` returns
219 performers sorted oldest-first).

### Stage 3: static site generator

Pages: `/`, `/shows/`, `/shows/<date>/`, `/performers/`, `/performers/<slug>/`.
Times rendered in ET and JST using `chrono-tz` (DST-correct).

Success criteria:
- `djdb build` writes `docs/` from current data.
- Snapshot tests for each page type against a fixture dataset.
- DST transition dates produce correct ET/JST offsets (test both spring and
  fall transitions).

Status: done. Maud templates, dark styled CSS, 6 tests covering
ET/JST conversion in EST + EDT + spring-forward gap + fall-back
ambiguity, plus a full build-to-tempdir test. Real build produces
63 show pages + 360 performer pages + 2 indexes + landing, matching
the source screenshots exactly.

### Stage 4: GitHub Actions + Pages deploy

Workflow: on push to main, run `cargo test`, `djdb check`, `djdb build`,
deploy `docs/` via `actions/deploy-pages`.

Success criteria: push triggers a successful deploy.

Status: done. `.github/workflows/build.yml` runs tests, validates data,
computes base URL (detects user vs project site), builds, and deploys
via actions/deploy-pages. Awaiting first push + manual Pages enablement
in repo settings to verify live.

### Stage 5: xlsx importer

One-shot command: `djdb import <path.xlsx>`. Reads every sheet (including
hidden) via `calamine`, extracts date from sheet title, parses the lineup
rows, fuzzy-matches performer names to existing slugs, writes show files and
stub performer entries. Prints a report of ambiguous matches for review.

Success criteria:
- Imports `data/kaleidosky.xlsx` producing valid show files that pass `check`.
- Ambiguous matches are reported, not silently guessed.

Status: done (stage reordered, ran before 2-4). 63 shows imported with
`--min-year 2025`. Pre-2025 sheets skipped due to out-of-order workbook
layout; will be addressed manually. Known junk performer entries
flagged for cleanup once `rename-performer` / merge commands land in
stage 2.

### Stage 6: desktop GUI (egui)

`src/bin/djdb-gui.rs` behind an optional `gui` feature so CI / headless
builds don't pay the eframe compile cost. Three tabs:

- Shows: left-list + in-pane editor (time/dur/dj/vj/notes table, add/
  remove, save, revert), plus "new show" date input.
- Performers: left-list with filter + form editor (name, twitch, cdn,
  aliases, notes) plus per-performer appearance history.
- Queries: stale list with days slider, last-played lookup.

Publish button runs `git add data/ && git commit && git push` so
non-CLI Windows users can ship changes without opening a terminal.

Status: done. Build with `cargo build --release --bin djdb-gui --features gui`.
