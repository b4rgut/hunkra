//! Small standalone settings, outside all user repositories.
use crate::i18n::Locale;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    pub locale: Locale,
    pub recent: Vec<PathBuf>,
}

/// Resolves the standalone config directory without involving repository configuration.
pub fn directory() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|base| base.join("hunkra"))
}
/// Loads bounded settings; invalid or missing settings use safe defaults.
pub fn load() -> Settings {
    let Some(directory) = directory() else {
        return Settings::default();
    };
    let Ok(file) = fs::File::open(directory.join("settings.json")) else {
        return Settings::default();
    };
    let mut bytes = Vec::new();
    if file.take(65_537).read_to_end(&mut bytes).is_err() || bytes.len() > 65_536 {
        return Settings::default();
    }
    serde_json::from_slice::<Settings>(&bytes)
        .map(|mut settings| {
            settings.recent.truncate(10);
            settings
        })
        .unwrap_or_default()
}
/// Atomically persists settings in the worker; a failed write preserves the old file.
pub fn save(settings: Settings) -> std::io::Result<()> {
    let directory = directory().ok_or_else(|| std::io::Error::other("no config directory"))?;
    fs::create_dir_all(&directory)?;
    let bytes = serde_json::to_vec_pretty(&settings)?;
    let temporary = directory.join(format!("settings-{}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, directory.join("settings.json"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
