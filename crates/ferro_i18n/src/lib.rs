//! Localization for FerroIDE, built on [Project Fluent](https://projectfluent.org).
//!
//! Translations live in `assets/locales/<lang>/*.ftl` — one file per area
//! of the UI (see [`FILES`]) — and are embedded in the binary. The active locale is process-global: every window shows the
//! same language, and switching it takes effect on the next frame.
//!
//! ```
//! use ferro_i18n::{Locale, set_locale, t};
//! set_locale(Locale::En);
//! assert_eq!(t("menu-file"), "File");
//! ```
//!
//! A missing key never panics: it falls back to English and then to the key
//! itself, so an untranslated string is visible rather than fatal.

use std::sync::{LazyLock, RwLock};

use fluent_bundle::{FluentArgs, FluentResource, FluentValue, concurrent::FluentBundle};
use serde::{Deserialize, Serialize};
use unic_langid::{LanguageIdentifier, langid};

/// Embed `assets/locales/<lang>/<name>.ftl` for both languages.
macro_rules! ftl_files {
    ($($name:literal),* $(,)?) => {
        /// The catalog files of every locale, in load order.
        pub const FILES: &[&str] = &[$($name),*];
        const EN_FTL: &[&str] = &[$(include_str!(concat!("../../../assets/locales/en/", $name, ".ftl"))),*];
        const PL_FTL: &[&str] = &[$(include_str!(concat!("../../../assets/locales/pl/", $name, ".ftl"))),*];
    };
}

ftl_files!(
    "ferro", "editor", "search", "panels", "vcs", "settings", "dialogs"
);

/// A language FerroIDE ships translations for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    #[default]
    En,
    Pl,
}

impl Locale {
    pub const ALL: [Locale; 2] = [Locale::En, Locale::Pl];

    /// The BCP 47 code, e.g. `en`.
    pub fn code(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Pl => "pl",
        }
    }

    /// The language's own name for itself, e.g. `Polski`.
    pub fn native_name(self) -> &'static str {
        match self {
            Locale::En => "English",
            Locale::Pl => "Polski",
        }
    }

    /// Parse a code such as `pl`, `pl-PL` or `pl_PL.UTF-8`.
    pub fn from_code(code: &str) -> Option<Self> {
        let lang = code
            .split(['-', '_', '.'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match lang.as_str() {
            "en" => Some(Locale::En),
            "pl" => Some(Locale::Pl),
            _ => None,
        }
    }

    /// Guess the user's locale from the environment (`LANG`, `LC_ALL`,
    /// `LC_MESSAGES`), defaulting to English.
    pub fn from_env() -> Self {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|var| std::env::var(var).ok())
            .find_map(|value| Self::from_code(&value))
            .unwrap_or_default()
    }

    fn langid(self) -> LanguageIdentifier {
        match self {
            Locale::En => langid!("en"),
            Locale::Pl => langid!("pl"),
        }
    }

    fn sources(self) -> &'static [&'static str] {
        match self {
            Locale::En => EN_FTL,
            Locale::Pl => PL_FTL,
        }
    }
}

struct Catalog {
    locale: Locale,
    active: FluentBundle<FluentResource>,
    fallback: FluentBundle<FluentResource>,
}

fn bundle(locale: Locale) -> FluentBundle<FluentResource> {
    let mut bundle = FluentBundle::new_concurrent(vec![locale.langid()]);
    // Bidi isolation marks render as boxes in many UI fonts.
    bundle.set_use_isolating(false);
    for (name, source) in FILES.iter().zip(locale.sources()) {
        let resource = FluentResource::try_new(source.to_string()).unwrap_or_else(|(_, errors)| {
            panic!("invalid FTL in {}/{name}.ftl: {errors:?}", locale.code())
        });
        bundle.add_resource(resource).unwrap_or_else(|errors| {
            panic!(
                "duplicate FTL ids in {}/{name}.ftl: {errors:?}",
                locale.code()
            )
        });
    }
    bundle
}

static CATALOG: LazyLock<RwLock<Catalog>> = LazyLock::new(|| {
    RwLock::new(Catalog {
        locale: Locale::En,
        active: bundle(Locale::En),
        fallback: bundle(Locale::En),
    })
});

/// The active locale.
pub fn locale() -> Locale {
    CATALOG.read().map(|c| c.locale).unwrap_or_default()
}

/// Switch the active locale for the whole process.
pub fn set_locale(locale: Locale) {
    if let Ok(mut catalog) = CATALOG.write()
        && catalog.locale != locale
    {
        catalog.active = bundle(locale);
        catalog.locale = locale;
    }
}

/// Translate a message id.
pub fn t(key: &str) -> String {
    format(key, None)
}

/// Translate a message id with named arguments, e.g.
/// `t_args("status-memory", &[("mb", 212.into())])`.
pub fn t_args(key: &str, args: &[(&str, Arg)]) -> String {
    let mut fluent_args = FluentArgs::new();
    for (name, value) in args {
        let value = match value {
            Arg::Str(s) => FluentValue::from(s.clone()),
            Arg::Num(n) => FluentValue::from(*n),
        };
        fluent_args.set(*name, value);
    }
    format(key, Some(&fluent_args))
}

/// A Fluent argument value.
#[derive(Debug, Clone)]
pub enum Arg {
    Str(String),
    Num(f64),
}

impl From<&str> for Arg {
    fn from(value: &str) -> Self {
        Arg::Str(value.to_string())
    }
}
impl From<String> for Arg {
    fn from(value: String) -> Self {
        Arg::Str(value)
    }
}
macro_rules! num_arg {
    ($($t:ty),*) => {$(
        impl From<$t> for Arg {
            fn from(value: $t) -> Self { Arg::Num(value as f64) }
        }
    )*};
}
num_arg!(i32, i64, u32, u64, usize, f32, f64);

fn format(key: &str, args: Option<&FluentArgs>) -> String {
    let Ok(catalog) = CATALOG.read() else {
        return key.to_string();
    };
    for bundle in [&catalog.active, &catalog.fallback] {
        if let Some(pattern) = bundle.get_message(key).and_then(|m| m.value()) {
            let mut errors = Vec::new();
            return bundle
                .format_pattern(pattern, args, &mut errors)
                .into_owned();
        }
    }
    key.to_string()
}

/// Message ids defined for `locale`, sorted. Used by tests and tooling to keep
/// the catalogs in sync.
pub fn message_ids(locale: Locale) -> Vec<String> {
    let mut ids: Vec<String> = locale
        .sources()
        .iter()
        .flat_map(|source| {
            let resource =
                FluentResource::try_new(source.to_string()).unwrap_or_else(|(res, _)| res);
            resource
                .entries()
                .filter_map(|entry| match entry {
                    fluent_syntax::ast::Entry::Message(message) => {
                        Some(message.id.name.to_string())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    ids.sort();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_parse_without_errors() {
        for locale in Locale::ALL {
            for (name, source) in FILES.iter().zip(locale.sources()) {
                FluentResource::try_new(source.to_string())
                    .unwrap_or_else(|(_, e)| panic!("{}/{name}: {e:?}", locale.code()));
            }
            // Duplicate ids across files panic here.
            let _ = bundle(locale);
        }
    }

    #[test]
    fn every_locale_defines_the_same_messages() {
        let en = message_ids(Locale::En);
        assert!(en.len() > 50, "the English catalog looks truncated");
        for locale in Locale::ALL {
            let ids = message_ids(locale);
            let missing: Vec<_> = en.iter().filter(|id| !ids.contains(id)).collect();
            let extra: Vec<_> = ids.iter().filter(|id| !en.contains(id)).collect();
            assert!(
                missing.is_empty() && extra.is_empty(),
                "{}: missing {missing:?}, extra {extra:?}",
                locale.code()
            );
        }
    }

    #[test]
    fn switching_locale_changes_translations_and_unknown_keys_fall_back() {
        set_locale(Locale::Pl);
        assert_eq!(t("menu-file"), "Plik");
        assert_eq!(t("no-such-key"), "no-such-key");
        assert_eq!(t_args("status-memory", &[("mb", 212.into())]), "212 MB");
        set_locale(Locale::En);
        assert_eq!(t("menu-file"), "File");
    }

    #[test]
    fn locale_codes_parse() {
        assert_eq!(Locale::from_code("pl_PL.UTF-8"), Some(Locale::Pl));
        assert_eq!(Locale::from_code("en-US"), Some(Locale::En));
        assert_eq!(Locale::from_code("de"), None);
    }
}
