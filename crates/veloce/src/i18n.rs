//! Interface language. English strings in the source are the message keys:
//! `tr("Refresh")` returns the Chinese text when that language is active and the
//! key itself otherwise, so a missing translation degrades to English instead of
//! showing a blank.

use std::{
    collections::HashMap,
    sync::{
        OnceLock,
        atomic::{AtomicU8, Ordering},
    },
};

mod zh;

/// A language the interface can be shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    English,
    Chinese,
}

/// What the user picked in Settings; `System` is resolved at apply time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguagePref {
    System,
    English,
    Chinese,
}

impl LanguagePref {
    pub const ALL: [LanguagePref; 3] = [
        LanguagePref::System,
        LanguagePref::English,
        LanguagePref::Chinese,
    ];

    /// Language names stay in their own language so a user who cannot read the
    /// current one can still find theirs.
    pub fn label(self) -> &'static str {
        match self {
            LanguagePref::System => tr("System"),
            LanguagePref::English => "English",
            LanguagePref::Chinese => "简体中文",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            LanguagePref::System => "system",
            LanguagePref::English => "en",
            LanguagePref::Chinese => "zh",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|pref| pref.key() == value)
    }

    pub fn resolve(self, system: Lang) -> Lang {
        match self {
            LanguagePref::System => system,
            LanguagePref::English => Lang::English,
            LanguagePref::Chinese => Lang::Chinese,
        }
    }
}

/// Pick a language from locale tags in order of preference (`zh-Hans-CN`,
/// `zh_CN.UTF-8`, `en-US`...). Only the primary subtag matters.
pub fn lang_from_tags<'a>(tags: impl IntoIterator<Item = &'a str>) -> Lang {
    for tag in tags {
        let primary = tag
            .trim()
            .split(['-', '_', '.'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match primary.as_str() {
            "zh" => return Lang::Chinese,
            "en" => return Lang::English,
            _ => {}
        }
    }
    Lang::English
}

/// The operating system's preferred language. A GUI app on macOS usually has no
/// `LANG`, so the global preference list is asked as well.
pub fn system_lang() -> Lang {
    let env: Vec<String> = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .filter(|value| !value.is_empty() && value != "C" && value != "POSIX")
        .collect();
    if !env.is_empty() {
        return lang_from_tags(env.iter().map(String::as_str));
    }
    #[cfg(target_os = "macos")]
    if let Ok(output) = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        let tags: Vec<&str> = text
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .filter(|word| !word.is_empty())
            .collect();
        return lang_from_tags(tags);
    }
    Lang::English
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == Lang::Chinese as u8 {
        Lang::Chinese
    } else {
        Lang::English
    }
}

fn zh_table() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| zh::TABLE.iter().copied().collect())
}

/// Translate a UI string. Unknown keys come back unchanged.
pub fn tr(key: &'static str) -> &'static str {
    translate(lang(), key)
}

/// Translate a template and fill its `{}` slots in order, so a language can put
/// the pieces in a different order than English does.
pub fn trf(key: &'static str, args: &[&str]) -> String {
    fill(tr(key), args)
}

fn fill(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut args = args.iter();
    let mut rest = template;
    while let Some(at) = rest.find("{}") {
        out.push_str(&rest[..at]);
        out.push_str(args.next().copied().unwrap_or(""));
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
}

fn translate(lang: Lang, key: &'static str) -> &'static str {
    match lang {
        Lang::English => key,
        Lang::Chinese => zh_table().get(key).copied().unwrap_or(key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_first_recognised_language_tag() {
        assert_eq!(lang_from_tags(["zh-Hans-CN", "en-US"]), Lang::Chinese);
        assert_eq!(lang_from_tags(["zh_CN.UTF-8"]), Lang::Chinese);
        assert_eq!(lang_from_tags(["en-US", "zh-Hans"]), Lang::English);
        assert_eq!(lang_from_tags(["fr-FR", "zh-TW"]), Lang::Chinese);
    }

    #[test]
    fn unknown_or_empty_locales_fall_back_to_english() {
        assert_eq!(lang_from_tags([]), Lang::English);
        assert_eq!(lang_from_tags(["fr-FR", "de"]), Lang::English);
        assert_eq!(lang_from_tags([""]), Lang::English);
    }

    #[test]
    fn a_preference_overrides_or_follows_the_system() {
        for system in [Lang::English, Lang::Chinese] {
            assert_eq!(LanguagePref::System.resolve(system), system);
            assert_eq!(LanguagePref::English.resolve(system), Lang::English);
            assert_eq!(LanguagePref::Chinese.resolve(system), Lang::Chinese);
        }
    }

    #[test]
    fn preference_keys_round_trip() {
        for pref in LanguagePref::ALL {
            assert_eq!(LanguagePref::parse(pref.key()), Some(pref));
        }
        assert_eq!(LanguagePref::parse("klingon"), None);
    }

    #[test]
    fn the_translation_table_has_no_blank_or_duplicate_entries() {
        let mut seen = std::collections::HashSet::new();
        for (en, zh) in zh::TABLE {
            assert!(!en.is_empty() && !zh.is_empty(), "blank entry for {en:?}");
            assert!(seen.insert(*en), "duplicate key {en:?}");
        }
    }

    #[test]
    fn slots_are_filled_in_order() {
        assert_eq!(fill("{} of {}", &["1", "2"]), "1 of 2");
        assert_eq!(fill("no slots", &["x"]), "no slots");
    }

    #[test]
    fn missing_arguments_leave_the_slot_empty() {
        assert_eq!(fill("a{}b{}c", &["1"]), "a1bc");
        assert_eq!(fill("", &[]), "");
    }

    #[test]
    fn translation_follows_the_requested_language() {
        // Not through the global: other tests read English labels concurrently.
        assert_eq!(translate(Lang::Chinese, "Refresh"), "刷新");
        assert_eq!(
            translate(Lang::Chinese, "no such key anywhere"),
            "no such key anywhere"
        );
        assert_eq!(translate(Lang::English, "Refresh"), "Refresh");
    }
}
