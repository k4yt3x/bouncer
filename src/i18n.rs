use std::borrow::Cow;
use std::collections::HashMap;

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Deserialize)]
pub struct Locale {
    pub start_button: String,
    pub welcome: String,
    pub welcome_no_group_name: String,
    pub generating_question: String,
    pub generating_button: String,
    pub question_intro: String,
    pub approved: String,
    pub rejected_wrong: String,
    pub rejected_timeout: String,
    pub rejected_llm_error: String,
    pub cooldown_notice: String,
}

#[derive(Debug, Clone)]
pub struct LocaleRegistry {
    locales: HashMap<String, Locale>,
    default: String,
}

impl LocaleRegistry {
    pub fn load(default: impl Into<String>) -> Result<Self> {
        let mut locales = HashMap::new();
        locales.insert(
            "en".to_string(),
            parse_locale("en", include_str!("locales/en.toml"))?,
        );
        locales.insert(
            "zh-CN".to_string(),
            parse_locale("zh-CN", include_str!("locales/zh-CN.toml"))?,
        );

        let default = default.into();
        if !locales.contains_key(&default) {
            return Err(Error::UnknownLocale(default));
        }
        Ok(Self { locales, default })
    }

    /// Resolve the locale for a group. Falls back to the default when the
    /// requested key is unknown.
    pub fn resolve(&self, preferred: Option<&str>) -> &Locale {
        if let Some(key) = preferred
            && let Some(locale) = self.locales.get(key)
        {
            return locale;
        }
        // `load` guarantees `self.default` exists.
        self.locales
            .get(&self.default)
            .expect("default locale must be registered")
    }

    pub fn is_known(&self, key: &str) -> bool {
        self.locales.contains_key(key)
    }
}

fn parse_locale(name: &str, raw: &str) -> Result<Locale> {
    toml::from_str(raw).map_err(|source| Error::LocaleParse {
        name: name.to_string(),
        source,
    })
}

/// Substitute `{placeholder}` occurrences in `template` with values from `args`.
/// Unknown placeholders are left in place so the output surfaces config gaps
/// instead of silently swallowing them.
pub fn render<'a>(template: &'a str, args: &[(&str, &str)]) -> Cow<'a, str> {
    if args.is_empty() || !template.contains('{') {
        return Cow::Borrowed(template);
    }
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let key = &after[..close];
                match args.iter().find(|(k, _)| *k == key) {
                    Some((_, value)) => out.push_str(value),
                    None => {
                        out.push('{');
                        out.push_str(key);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_known_placeholders() {
        let result = render("hi {name}, it is {day}", &[("name", "bob"), ("day", "fri")]);
        assert_eq!(result, "hi bob, it is fri");
    }

    #[test]
    fn render_leaves_unknown_placeholders() {
        let result = render("hi {name}, it is {day}", &[("name", "bob")]);
        assert_eq!(result, "hi bob, it is {day}");
    }

    #[test]
    fn builtin_locales_load() {
        let registry = LocaleRegistry::load("en").expect("en loads");
        assert!(registry.is_known("en"));
        assert!(registry.is_known("zh-CN"));
        assert!(!registry.resolve(Some("en")).start_button.is_empty());
        assert!(!registry.resolve(Some("zh-CN")).start_button.is_empty());
        assert!(!registry.resolve(Some("xx")).start_button.is_empty());
    }
}
