use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GmailSourceConfig {
    pub credentials_path: String,
    pub token_dir: String,
    pub account_key: String,
    #[serde(default)]
    pub display_email: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoriaConfig {
    #[serde(default)]
    pub recent_archives: Vec<String>,
    #[serde(default)]
    pub default_archive: Option<String>,
    #[serde(default)]
    pub gmail_sources: BTreeMap<String, GmailSourceConfig>,
}

impl MemoriaConfig {
    pub fn standard_path() -> Option<PathBuf> {
        dirs::config_dir().map(|path| path.join("memoria/config.json"))
    }

    pub fn load() -> io::Result<Self> {
        Self::standard_path()
            .map(|path| Self::load_from(&path))
            .unwrap_or_else(|| Ok(Self::default()))
    }

    pub fn load_from(path: &Path) -> io::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(path)?;
        serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(path) = Self::standard_path() {
            self.save_to(&path)
        } else {
            Ok(())
        }
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        fs::write(path, bytes)
    }

    pub fn remember_archive(&mut self, archive: &Path) {
        let value = archive.to_string_lossy().into_owned();
        self.recent_archives.retain(|entry| entry != &value);
        self.recent_archives.insert(0, value.clone());
        self.recent_archives.truncate(8);
        self.default_archive = Some(value);
    }

    pub fn source_for(&self, archive: &Path) -> Option<GmailSourceConfig> {
        self.gmail_sources
            .get(&archive.to_string_lossy().into_owned())
            .cloned()
    }

    pub fn set_source(&mut self, archive: &Path, source: GmailSourceConfig) {
        self.gmail_sources
            .insert(archive.to_string_lossy().into_owned(), source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_roundtrips_without_secrets() {
        let path = std::env::temp_dir().join(format!("memoria-config-{}.json", std::process::id()));
        let mut config = MemoriaConfig::default();
        let archive = Path::new("/tmp/archive");
        config.remember_archive(archive);
        config.set_source(
            archive,
            GmailSourceConfig {
                credentials_path: "/tmp/client.json".into(),
                token_dir: "/tmp/tokens".into(),
                account_key: "gmail:fixture".into(),
                display_email: Some("fixture@example.test".into()),
            },
        );
        config.save_to(&path).unwrap();
        let loaded = MemoriaConfig::load_from(&path).unwrap();
        assert_eq!(loaded, config);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("access_token"));
        let _ = std::fs::remove_file(path);
    }
}
