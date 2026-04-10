use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A performer (DJ or VJ). Keyed by slug in `performers.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Performer {
    pub display_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub twitch: String,
    #[serde(default)]
    pub cdn: String,
}

pub type Slug = String;

/// All performers, keyed by slug. BTreeMap for stable ordering on write.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Performers(pub BTreeMap<Slug, Performer>);

#[derive(Debug, Error)]
pub enum PerformerError {
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
    #[error("performer slug {slug:?} is empty or contains invalid characters (use a-z, 0-9, underscore)")]
    InvalidSlug { slug: String },
    #[error("performer {slug:?} has empty display_name")]
    EmptyDisplayName { slug: String },
}

impl Performers {
    pub fn load(path: &Path) -> Result<Self, PerformerError> {
        let text = std::fs::read_to_string(path).map_err(|source| PerformerError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let performers: Performers =
            toml::from_str(&text).map_err(|source| PerformerError::Parse {
                path: path.display().to_string(),
                source,
            })?;
        performers.validate()?;
        Ok(performers)
    }

    pub fn validate(&self) -> Result<(), PerformerError> {
        for (slug, p) in &self.0 {
            if !is_valid_slug(slug) {
                return Err(PerformerError::InvalidSlug { slug: slug.clone() });
            }
            if p.display_name.trim().is_empty() {
                return Err(PerformerError::EmptyDisplayName { slug: slug.clone() });
            }
        }
        Ok(())
    }

    pub fn contains(&self, slug: &str) -> bool {
        self.0.contains_key(slug)
    }

    pub fn get(&self, slug: &str) -> Option<&Performer> {
        self.0.get(slug)
    }
}

pub fn is_valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_slugs() {
        assert!(is_valid_slug("pizza_princess"));
        assert!(is_valid_slug("dj_1"));
        assert!(is_valid_slug("a"));
    }

    #[test]
    fn invalid_slugs() {
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("Pizza"));
        assert!(!is_valid_slug("pizza-princess"));
        assert!(!is_valid_slug("pizza princess"));
        assert!(!is_valid_slug("ピザ"));
    }

    #[test]
    fn parse_performers() {
        let text = r#"
[pizza_princess]
display_name = "Pizza Princess"

[goblin_mode]
display_name = "Goblin mode"
aliases = ["GoblinMode"]
notes = "host"
twitch = "https://twitch.tv/goblin"
"#;
        let p: Performers = toml::from_str(text).unwrap();
        p.validate().unwrap();
        assert_eq!(p.0.len(), 2);
        assert_eq!(p.get("goblin_mode").unwrap().aliases, vec!["GoblinMode"]);
    }

    #[test]
    fn rejects_invalid_slug() {
        let text = r#"
[Pizza]
display_name = "Pizza"
"#;
        let p: Performers = toml::from_str(text).unwrap();
        assert!(matches!(
            p.validate(),
            Err(PerformerError::InvalidSlug { .. })
        ));
    }

    #[test]
    fn rejects_empty_display_name() {
        let text = r#"
[pizza]
display_name = ""
"#;
        let p: Performers = toml::from_str(text).unwrap();
        assert!(matches!(
            p.validate(),
            Err(PerformerError::EmptyDisplayName { .. })
        ));
    }
}
