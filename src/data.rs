use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::performer::{PerformerError, Performers};
use crate::show::{Show, ShowError};

/// A loaded and fully cross-validated dataset.
#[derive(Debug)]
pub struct Dataset {
    pub performers: Performers,
    pub shows: Vec<Show>,
}

#[derive(Debug, Error)]
pub enum DatasetError {
    #[error(transparent)]
    Performer(#[from] PerformerError),
    #[error(transparent)]
    Show(#[from] ShowError),
    #[error("listing shows directory {path}: {source}")]
    ListShows {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("show {date} references unknown performer slug {slug:?} (set at {time})")]
    UnknownPerformer {
        date: chrono::NaiveDate,
        time: String,
        slug: String,
    },
    #[error("duplicate show for date {date} ({a} and {b})")]
    DuplicateShow {
        date: chrono::NaiveDate,
        a: String,
        b: String,
    },
}

impl Dataset {
    /// Load from a `data/` directory containing `performers.toml` and `shows/`.
    pub fn load(root: &Path) -> Result<Self, DatasetError> {
        let performers = Performers::load(&root.join("performers.toml"))?;

        let shows_dir = root.join("shows");
        let mut shows: Vec<(PathBuf, Show)> = Vec::new();
        if shows_dir.exists() {
            let entries =
                std::fs::read_dir(&shows_dir).map_err(|source| DatasetError::ListShows {
                    path: shows_dir.display().to_string(),
                    source,
                })?;
            for entry in entries {
                let entry = entry.map_err(|source| DatasetError::ListShows {
                    path: shows_dir.display().to_string(),
                    source,
                })?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                    continue;
                }
                let show = Show::load(&path)?;
                shows.push((path, show));
            }
        }

        // Sort by date for stable ordering.
        shows.sort_by_key(|(_, s)| s.date);

        // Reject duplicate dates.
        for pair in shows.windows(2) {
            if pair[0].1.date == pair[1].1.date {
                return Err(DatasetError::DuplicateShow {
                    date: pair[0].1.date,
                    a: pair[0].0.display().to_string(),
                    b: pair[1].0.display().to_string(),
                });
            }
        }

        // Cross-validate performer references.
        for (_, show) in &shows {
            for set in &show.sets {
                for slug in [set.dj.as_ref(), set.vj.as_ref()].into_iter().flatten() {
                    if !performers.contains(slug) {
                        return Err(DatasetError::UnknownPerformer {
                            date: show.date,
                            time: format!("{:02}:{:02}", set.start.hour, set.start.minute),
                            slug: slug.clone(),
                        });
                    }
                }
            }
        }

        Ok(Dataset {
            performers,
            shows: shows.into_iter().map(|(_, s)| s).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn valid_fixture(dir: &Path) {
        write(
            dir,
            "performers.toml",
            r#"
[aliquem]
display_name = "Aliquem"

[kohada]
display_name = "Kohada"
"#,
        );
        write(
            dir,
            "shows/2025-03-28.toml",
            r#"
date = "2025-03-28"

[[sets]]
start = "20:00"
dj = "aliquem"

[[sets]]
start = "21:00"
dj = "kohada"
"#,
        );
    }

    #[test]
    fn loads_valid_dataset() {
        let dir = tempdir().unwrap();
        valid_fixture(dir.path());
        let ds = Dataset::load(dir.path()).unwrap();
        assert_eq!(ds.performers.0.len(), 2);
        assert_eq!(ds.shows.len(), 1);
    }

    #[test]
    fn rejects_unknown_performer() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "performers.toml",
            r#"
[aliquem]
display_name = "Aliquem"
"#,
        );
        write(
            dir.path(),
            "shows/2025-03-28.toml",
            r#"
date = "2025-03-28"

[[sets]]
start = "20:00"
dj = "ghost"
"#,
        );
        let err = Dataset::load(dir.path()).unwrap_err();
        assert!(matches!(err, DatasetError::UnknownPerformer { .. }));
    }

    #[test]
    fn rejects_filename_mismatch() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "performers.toml",
            r#"
[aliquem]
display_name = "Aliquem"
"#,
        );
        write(
            dir.path(),
            "shows/2025-03-28.toml",
            r#"
date = "2025-04-01"

[[sets]]
start = "20:00"
dj = "aliquem"
"#,
        );
        let err = Dataset::load(dir.path()).unwrap_err();
        assert!(matches!(
            err,
            DatasetError::Show(ShowError::DateFilenameMismatch { .. })
        ));
    }

    #[test]
    fn missing_shows_dir_is_ok() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "performers.toml",
            r#"
[aliquem]
display_name = "Aliquem"
"#,
        );
        let ds = Dataset::load(dir.path()).unwrap();
        assert_eq!(ds.shows.len(), 0);
    }
}
