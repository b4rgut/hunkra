//! Complete offline Fluent catalogs and a small Russian terminology overlay.
use fluent_bundle::{FluentArgs, FluentBundle, FluentResource};
use fluent_syntax::{ast, parser};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const EN: &str = include_str!("../locales/en.ftl");
const RU: &str = include_str!("../locales/ru.ftl");
const DEV: &str = include_str!("../locales/ru-dev.ftl");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Locale {
    En,
    Ru,
    #[default]
    RuDev,
}

impl Locale {
    /// Returns the stable setting identifier, independent of the translated label.
    pub fn label(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Ru => "Русский",
            Self::RuDev => "Русский (программистский)",
        }
    }
}

pub struct Translator {
    bundle: FluentBundle<FluentResource>,
    fallback: FluentBundle<FluentResource>,
}

/// Parses trusted embedded resources; malformed shipping catalogs are a build/test failure.
fn resource(text: &str) -> FluentResource {
    FluentResource::try_new(text.to_owned()).expect("validated bundled Fluent resource")
}

impl Translator {
    /// Uses ru locale rules for both Russian profiles, including their numeric forms.
    pub fn new(locale: Locale) -> Self {
        let language = if locale == Locale::En { "en" } else { "ru" };
        let mut bundle = FluentBundle::new(vec![language.parse().expect("valid locale")]);
        bundle
            .add_resource(resource(if locale == Locale::En { EN } else { RU }))
            .expect("unique keys");
        if locale == Locale::RuDev {
            bundle.add_resource_overriding(resource(DEV));
        }
        let mut fallback = FluentBundle::new(vec!["en".parse().expect("valid locale")]);
        fallback.add_resource(resource(EN)).expect("unique keys");
        // Values such as paths and hashes are isolated at the UI boundary; numbers stay readable.
        bundle.set_use_isolating(false);
        fallback.set_use_isolating(false);
        Self { bundle, fallback }
    }
    /// Formats an owned application message, falling back explicitly to the English catalog.
    pub fn text(&self, key: &str, args: Option<&FluentArgs<'_>>) -> String {
        for bundle in [&self.bundle, &self.fallback] {
            if let Some(pattern) = bundle.get_message(key).and_then(|message| message.value()) {
                let mut errors = Vec::new();
                let text = bundle.format_pattern(pattern, args, &mut errors);
                if errors.is_empty() {
                    return text.into_owned();
                }
            }
        }
        "Translation unavailable".to_owned()
    }
    /// Formats file counts through Fluent grammar rather than string concatenation.
    pub fn count(&self, count: usize) -> String {
        let mut args = FluentArgs::new();
        args.set("count", count);
        self.text("file-count", Some(&args))
    }
}

/// Collects arguments throughout selectors, nested placeables and function calls.
fn inline_args(value: &ast::InlineExpression<&str>, args: &mut BTreeSet<String>) {
    match value {
        ast::InlineExpression::VariableReference { id } => {
            args.insert(id.name.to_owned());
        }
        ast::InlineExpression::Placeable { expression } => expression_args(expression, args),
        ast::InlineExpression::FunctionReference { arguments, .. } => call_args(arguments, args),
        ast::InlineExpression::TermReference {
            arguments: Some(arguments),
            ..
        } => call_args(arguments, args),
        _ => {}
    }
}
/// Visits positional and named function/term arguments.
fn call_args(arguments: &ast::CallArguments<&str>, args: &mut BTreeSet<String>) {
    for argument in &arguments.positional {
        inline_args(argument, args);
    }
    for argument in &arguments.named {
        inline_args(&argument.value, args);
    }
}
/// Visits selectors as well as every translated branch.
fn expression_args(value: &ast::Expression<&str>, args: &mut BTreeSet<String>) {
    match value {
        ast::Expression::Inline(inline) => inline_args(inline, args),
        ast::Expression::Select { selector, variants } => {
            inline_args(selector, args);
            for variant in variants {
                pattern_args(&variant.value, args);
            }
        }
    }
}
/// Collects the signature of one message or attribute pattern.
fn pattern_args(pattern: &ast::Pattern<&str>, args: &mut BTreeSet<String>) {
    for element in &pattern.elements {
        if let ast::PatternElement::Placeable { expression } = element {
            expression_args(expression, args);
        }
    }
}
/// Rejects duplicates and compares each message/attribute's argument signature.
fn signatures(source: &str) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let parsed =
        parser::parse(source).map_err(|(_, errors)| format!("Fluent syntax: {errors:?}"))?;
    let mut result = BTreeMap::new();
    for entry in parsed.body {
        if let ast::Entry::Message(message) = entry {
            let mut args = BTreeSet::new();
            if let Some(pattern) = &message.value {
                pattern_args(pattern, &mut args);
            }
            if result.insert(message.id.name.to_owned(), args).is_some() {
                return Err(format!("duplicate key {}", message.id.name));
            }
            for attribute in message.attributes {
                let mut args = BTreeSet::new();
                pattern_args(&attribute.value, &mut args);
                let key = format!("{}.{}", message.id.name, attribute.id.name);
                if result.insert(key.clone(), args).is_some() {
                    return Err(format!("duplicate attribute {key}"));
                }
            }
        }
    }
    Ok(result)
}
/// Checks both complete catalogs and each overlay key/signature without needing GPUI.
pub fn check_catalogs() -> Result<(), String> {
    let en = signatures(EN)?;
    let ru = signatures(RU)?;
    let overlay = signatures(DEV)?;
    if en != ru {
        return Err("en/ru message keys or argument signatures differ".to_owned());
    }
    for (key, args) in &overlay {
        if ru.get(key) != Some(args) {
            return Err(format!("overlay key/signature: {key}"));
        }
    }
    if overlay.len() >= ru.len() {
        return Err("overlay duplicates the whole catalog".to_owned());
    }
    for locale in [Locale::En, Locale::Ru, Locale::RuDev] {
        let translator = Translator::new(locale);
        for (key, variables) in &en {
            let mut args = FluentArgs::new();
            for variable in variables {
                args.set(variable, 2);
            }
            if translator.text(key, Some(&args)) == "Translation unavailable" {
                return Err(format!("format failed: {locale:?}/{key}"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    /// Checks complete catalogs, overlay signatures and every formatted message.
    fn complete_catalogs() {
        check_catalogs().unwrap();
    }
    #[test]
    /// Verifies dependent grammatical phrases and natural Russian nouns.
    fn grammar_and_overlay() {
        let ru = Translator::new(Locale::Ru);
        let dev = Translator::new(Locale::RuDev);
        assert_eq!(ru.text("commit", None), "Фиксация");
        assert_eq!(dev.text("commit", None), "Коммит");
        assert_eq!(ru.text("commit-created", None), "Фиксация создана");
        assert_eq!(dev.text("commit-created", None), "Коммит создан");
        assert_eq!(dev.text("branch", None), "Ветка");
    }
    #[test]
    /// Verifies Russian CLDR plurals for both Russian profiles and English fallback.
    fn plural_forms() {
        for locale in [Locale::Ru, Locale::RuDev] {
            let t = Translator::new(locale);
            for (count, text) in [
                (1, "1 файл"),
                (2, "2 файла"),
                (5, "5 файлов"),
                (11, "11 файлов"),
                (21, "21 файл"),
                (22, "22 файла"),
                (25, "25 файлов"),
            ] {
                assert_eq!(t.count(count), text);
            }
        }
        assert_eq!(Translator::new(Locale::En).count(1), "1 file");
        assert_eq!(Translator::new(Locale::En).count(2), "2 files");
    }
}
