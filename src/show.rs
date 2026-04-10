use std::path::Path;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::performer::Slug;

/// Wall-clock time on the show's ET date, as `HH:MM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallTime {
    pub hour: u8,
    pub minute: u8,
}

impl WallTime {
    pub fn parse(s: &str) -> Result<Self, ShowError> {
        let (h, m) = s
            .split_once(':')
            .ok_or_else(|| ShowError::BadTime { value: s.to_string() })?;
        let hour: u8 = h
            .parse()
            .map_err(|_| ShowError::BadTime { value: s.to_string() })?;
        let minute: u8 = m
            .parse()
            .map_err(|_| ShowError::BadTime { value: s.to_string() })?;
        if hour > 23 || minute > 59 {
            return Err(ShowError::BadTime { value: s.to_string() });
        }
        Ok(WallTime { hour, minute })
    }
}

impl Serialize for WallTime {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{:02}:{:02}", self.hour, self.minute))
    }
}

impl<'de> Deserialize<'de> for WallTime {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        WallTime::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// One performer slot in a show.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Set {
    pub start: WallTime,
    /// Duration in minutes. Defaults to 60 when omitted.
    #[serde(default = "default_duration")]
    pub duration_min: u32,
    #[serde(default)]
    pub dj: Option<Slug>,
    #[serde(default)]
    pub vj: Option<Slug>,
    #[serde(default)]
    pub notes: String,
}

fn default_duration() -> u32 {
    60
}

/// A single night's lineup. `date` is the ET calendar date.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Show {
    pub date: NaiveDate,
    #[serde(default)]
    pub sets: Vec<Set>,
}

#[derive(Debug, Error)]
pub enum ShowError {
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("show file {path}: date {file_date} does not match filename")]
    DateFilenameMismatch { path: String, file_date: NaiveDate },
    #[error("show file {path}: filename is not a YYYY-MM-DD date")]
    BadFilename { path: String },
    #[error("show {date}: has no sets")]
    EmptyLineup { date: NaiveDate },
    #[error("show {date}: set at {time} has neither dj nor vj")]
    EmptySet { date: NaiveDate, time: String },
    #[error("bad time value {value:?}")]
    BadTime { value: String },
}

impl Show {
    pub fn load(path: &Path) -> Result<Self, ShowError> {
        let text = std::fs::read_to_string(path).map_err(|source| ShowError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let show: Show = toml::from_str(&text).map_err(|source| ShowError::Parse {
            path: path.display().to_string(),
            source,
        })?;

        // Filename must match internal date.
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| ShowError::BadFilename {
                path: path.display().to_string(),
            })?;
        let filename_date = NaiveDate::parse_from_str(stem, "%Y-%m-%d").map_err(|_| {
            ShowError::BadFilename {
                path: path.display().to_string(),
            }
        })?;
        if filename_date != show.date {
            return Err(ShowError::DateFilenameMismatch {
                path: path.display().to_string(),
                file_date: show.date,
            });
        }

        show.validate_shape()?;
        Ok(show)
    }

    fn validate_shape(&self) -> Result<(), ShowError> {
        if self.sets.is_empty() {
            return Err(ShowError::EmptyLineup { date: self.date });
        }
        for set in &self.sets {
            if set.dj.is_none() && set.vj.is_none() {
                return Err(ShowError::EmptySet {
                    date: self.date,
                    time: format!("{:02}:{:02}", set.start.hour, set.start.minute),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_walltime() {
        assert_eq!(WallTime::parse("20:00").unwrap(), WallTime { hour: 20, minute: 0 });
        assert_eq!(WallTime::parse("00:59").unwrap(), WallTime { hour: 0, minute: 59 });
    }

    #[test]
    fn bad_walltime() {
        assert!(WallTime::parse("24:00").is_err());
        assert!(WallTime::parse("12:60").is_err());
        assert!(WallTime::parse("1200").is_err());
        assert!(WallTime::parse("abc").is_err());
    }

    #[test]
    fn parse_show() {
        let text = r#"
date = "2025-03-28"

[[sets]]
start = "20:00"
dj = "aliquem"
notes = "takeover"

[[sets]]
start = "21:00"
dj = "kohada"

[[sets]]
start = "22:00"
duration_min = 90
dj = "turels"
vj = "some_vj"
"#;
        let s: Show = toml::from_str(text).unwrap();
        assert_eq!(s.sets.len(), 3);
        assert_eq!(s.sets[0].duration_min, 60);
        assert_eq!(s.sets[2].duration_min, 90);
        s.validate_shape().unwrap();
    }

    #[test]
    fn empty_set_rejected() {
        let text = r#"
date = "2025-03-28"

[[sets]]
start = "20:00"
"#;
        let s: Show = toml::from_str(text).unwrap();
        assert!(matches!(s.validate_shape(), Err(ShowError::EmptySet { .. })));
    }

    #[test]
    fn empty_lineup_rejected() {
        let text = r#"date = "2025-03-28""#;
        let s: Show = toml::from_str(text).unwrap();
        assert!(matches!(
            s.validate_shape(),
            Err(ShowError::EmptyLineup { .. })
        ));
    }
}
