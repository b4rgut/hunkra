//! Small standalone settings, outside all user repositories.
use crate::i18n::Locale;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
};

/// Stores an explicit language or follows the operating system on every launch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    System,
    En,
    Ru,
    RuDev,
}

impl Language {
    /// Resolves a persisted preference without changing the process or repository environment.
    pub fn resolve(self) -> Locale {
        match self {
            Self::System => system_locale(),
            Self::En => Locale::En,
            Self::Ru => Locale::Ru,
            Self::RuDev => Locale::RuDev,
        }
    }
    /// Converts an explicitly chosen profile while retaining legacy settings identifiers.
    pub fn explicit(locale: Locale) -> Self {
        match locale {
            Locale::En => Self::En,
            Locale::Ru => Self::Ru,
            Locale::RuDev => Self::RuDev,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub locale: Language,
    pub recent: Vec<PathBuf>,
    pub diff_details: bool,
}

/// Maps Linux desktop message preferences to bundled catalogs, preferring programmer Russian.
fn locale_from_environment(mut variable: impl FnMut(&str) -> Option<String>) -> Locale {
    let base = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| variable(name).filter(|value| !value.trim().is_empty()))
        .unwrap_or_else(|| "C".into());
    if matches!(base.split('.').next(), Some("C" | "POSIX")) {
        return Locale::En;
    }
    let preferences = variable("LANGUAGE").unwrap_or_default();
    for candidate in preferences.split(':').chain(std::iter::once(base.as_str())) {
        match candidate.trim().split(['_', '-', '.', '@']).next() {
            Some(language) if language.eq_ignore_ascii_case("ru") => return Locale::RuDev,
            Some(language) if language.eq_ignore_ascii_case("en") => return Locale::En,
            _ => {}
        }
    }
    Locale::En
}

/// Reads the OS language at startup or when the user selects the system preference.
pub fn system_locale() -> Locale {
    locale_from_environment(|name| std::env::var(name).ok())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Checks desktop preference order, C locales and the Russian terminology priority without mutating global environment.
    fn system_language_order() {
        let cases: &[(&[(&str, &str)], Locale)] = &[
            (&[("LANG", "ru_RU.UTF-8")], Locale::RuDev),
            (&[("LANG", "RU-ru")], Locale::RuDev),
            (&[("LANG", "en_US.UTF-8")], Locale::En),
            (&[("LANG", "ru_RU"), ("LC_MESSAGES", "en_GB")], Locale::En),
            (&[("LANG", "en_US"), ("LC_ALL", "ru_RU")], Locale::RuDev),
            (
                &[("LANG", "en_US"), ("LANGUAGE", "de:ru:en")],
                Locale::RuDev,
            ),
            (&[("LANG", "ru_RU"), ("LANGUAGE", "en:ru")], Locale::En),
            (
                &[("LANG", "ru_RU"), ("LC_ALL", ""), ("LC_MESSAGES", "")],
                Locale::RuDev,
            ),
            (
                &[("LANG", "ru_RU"), ("LC_ALL", "C.UTF-8"), ("LANGUAGE", "ru")],
                Locale::En,
            ),
            (&[("LANG", "POSIX"), ("LANGUAGE", "ru")], Locale::En),
            (&[("LANG", "de_DE.UTF-8")], Locale::En),
            (&[], Locale::En),
        ];
        for (environment, expected) in cases {
            assert_eq!(
                locale_from_environment(|name| environment
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).into())),
                *expected,
                "{environment:?}"
            );
        }
    }

    #[test]
    /// Loads old explicit preferences and new partial settings without losing recent repositories.
    fn legacy_and_default_preferences() {
        let empty: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.locale, Language::System);
        assert!(!empty.diff_details);
        for (name, locale) in [
            ("En", Locale::En),
            ("Ru", Locale::Ru),
            ("RuDev", Locale::RuDev),
        ] {
            let stored = format!(r#"{{"locale":"{name}","recent":["/fixture"]}}"#);
            let settings: Settings = serde_json::from_str(&stored).unwrap();
            assert_eq!(settings.locale.resolve(), locale);
            assert_eq!(settings.recent, [PathBuf::from("/fixture")]);
            assert_eq!(
                serde_json::from_slice::<Settings>(&serde_json::to_vec(&settings).unwrap())
                    .unwrap()
                    .locale,
                settings.locale
            );
        }
    }
}
