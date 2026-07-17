//! Private localization boundary for Pop Lang tool presentation.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;

use pop_foundation::{Diagnostic, DiagnosticArgument, DiagnosticSeverity};
use serde::Deserialize;

struct CatalogSource {
    manifest: &'static str,
    fragments: &'static [(&'static str, &'static str)],
}

macro_rules! catalog_source {
    ($locale:literal) => {
        CatalogSource {
            manifest: include_str!(concat!(
                "../../../../assets/locales/",
                $locale,
                "/manifest.toml"
            )),
            fragments: &[
                (
                    "cli/errors.toml",
                    include_str!(concat!(
                        "../../../../assets/locales/",
                        $locale,
                        "/cli/errors.toml"
                    )),
                ),
                (
                    "cli/help.toml",
                    include_str!(concat!(
                        "../../../../assets/locales/",
                        $locale,
                        "/cli/help.toml"
                    )),
                ),
                (
                    "compiler/backend.toml",
                    include_str!(concat!(
                        "../../../../assets/locales/",
                        $locale,
                        "/compiler/backend.toml"
                    )),
                ),
                (
                    "compiler/compile-time.toml",
                    include_str!(concat!(
                        "../../../../assets/locales/",
                        $locale,
                        "/compiler/compile-time.toml"
                    )),
                ),
                (
                    "compiler/documentation.toml",
                    include_str!(concat!(
                        "../../../../assets/locales/",
                        $locale,
                        "/compiler/documentation.toml"
                    )),
                ),
                (
                    "compiler/ffi.toml",
                    include_str!(concat!(
                        "../../../../assets/locales/",
                        $locale,
                        "/compiler/ffi.toml"
                    )),
                ),
                (
                    "compiler/fixes.toml",
                    include_str!(concat!(
                        "../../../../assets/locales/",
                        $locale,
                        "/compiler/fixes.toml"
                    )),
                ),
                (
                    "compiler/lexer.toml",
                    include_str!(concat!(
                        "../../../../assets/locales/",
                        $locale,
                        "/compiler/lexer.toml"
                    )),
                ),
                (
                    "compiler/parser.toml",
                    include_str!(concat!(
                        "../../../../assets/locales/",
                        $locale,
                        "/compiler/parser.toml"
                    )),
                ),
                (
                    "compiler/resolution.toml",
                    include_str!(concat!(
                        "../../../../assets/locales/",
                        $locale,
                        "/compiler/resolution.toml"
                    )),
                ),
                (
                    "compiler/types.toml",
                    include_str!(concat!(
                        "../../../../assets/locales/",
                        $locale,
                        "/compiler/types.toml"
                    )),
                ),
                (
                    "lsp/session.toml",
                    include_str!(concat!(
                        "../../../../assets/locales/",
                        $locale,
                        "/lsp/session.toml"
                    )),
                ),
                (
                    "shared/presentation.toml",
                    include_str!(concat!(
                        "../../../../assets/locales/",
                        $locale,
                        "/shared/presentation.toml"
                    )),
                ),
            ],
        }
    };
}

const ENGLISH: CatalogSource = catalog_source!("en");
const SIMPLIFIED_CHINESE: CatalogSource = catalog_source!("zh-Hans");
const JAPANESE: CatalogSource = catalog_source!("ja");
const PORTUGUESE_BRAZIL: CatalogSource = catalog_source!("pt-BR");
const SPANISH: CatalogSource = catalog_source!("es");

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Language {
    English,
    SimplifiedChinese,
    Japanese,
    PortugueseBrazil,
    Spanish,
}

impl Language {
    pub const ALL: [Self; 5] = [
        Self::English,
        Self::SimplifiedChinese,
        Self::Japanese,
        Self::PortugueseBrazil,
        Self::Spanish,
    ];

    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::SimplifiedChinese => "zh-Hans",
            Self::Japanese => "ja",
            Self::PortugueseBrazil => "pt-BR",
            Self::Spanish => "es",
        }
    }

    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        let tag = normalize_tag(tag);
        let lower = tag.to_ascii_lowercase();
        if lower == "en" || lower.starts_with("en-") {
            Some(Self::English)
        } else if matches!(lower.as_str(), "zh" | "zh-cn" | "zh-sg" | "zh-hans")
            || lower.starts_with("zh-hans-")
        {
            Some(Self::SimplifiedChinese)
        } else if lower == "ja" || lower.starts_with("ja-") {
            Some(Self::Japanese)
        } else if lower == "pt" || lower == "pt-br" {
            Some(Self::PortugueseBrazil)
        } else if lower == "es" || lower.starts_with("es-") {
            Some(Self::Spanish)
        } else {
            None
        }
    }
}

fn normalize_tag(tag: &str) -> String {
    tag.split(['.', '@'])
        .next()
        .unwrap_or(tag)
        .replace('_', "-")
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LanguageSources {
    pub explicit: Option<String>,
    pub environment: Option<String>,
    pub config: Option<String>,
    pub system: Option<String>,
    pub config_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalizationError {
    UnsupportedLanguage(String),
    InvalidConfiguration(String),
    InvalidCatalog(String),
    UnknownMessage(String),
    InvalidArguments(String),
}

impl fmt::Display for LocalizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedLanguage(tag) => write!(formatter, "unsupported language `{tag}`"),
            Self::InvalidConfiguration(reason) => {
                write!(formatter, "invalid tool configuration: {reason}")
            }
            Self::InvalidCatalog(reason) => {
                write!(formatter, "invalid localization catalog: {reason}")
            }
            Self::UnknownMessage(key) => write!(formatter, "unknown localization message `{key}`"),
            Self::InvalidArguments(reason) => formatter.write_str(reason),
        }
    }
}

impl std::error::Error for LocalizationError {}

/// Selects a supported language from explicit, environment, configuration, and
/// system inputs in precedence order.
///
/// # Errors
///
/// Returns [`LocalizationError::UnsupportedLanguage`] when an explicit,
/// environment, or configuration tag is not supported.
pub fn select_language(sources: &LanguageSources) -> Result<Language, LocalizationError> {
    if let Some(tag) = [
        sources.explicit.as_deref(),
        sources.environment.as_deref(),
        sources.config.as_deref(),
    ]
    .into_iter()
    .flatten()
    .next()
    {
        return Language::from_tag(tag)
            .ok_or_else(|| LocalizationError::UnsupportedLanguage(tag.to_owned()));
    }
    Ok(sources
        .system
        .as_deref()
        .and_then(Language::from_tag)
        .unwrap_or(Language::English))
}

#[derive(Deserialize)]
struct ToolConfiguration {
    language: Option<String>,
}

/// Selects the language for one tool process using the accepted precedence.
///
/// # Errors
///
/// Returns an error for an unsupported selected tag or malformed/read-failing
/// user configuration.
pub fn select_process_language(explicit: Option<&str>) -> Result<Language, LocalizationError> {
    let config_path = user_config_path();
    let config = config_path
        .as_deref()
        .and_then(|path| match std::fs::read_to_string(path) {
            Ok(source) => Some(
                toml::from_str::<ToolConfiguration>(&source)
                    .map_err(|error| {
                        LocalizationError::InvalidConfiguration(format!(
                            "{}: {error}",
                            path.display()
                        ))
                    })
                    .map(|configuration| configuration.language),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(Err(LocalizationError::InvalidConfiguration(format!(
                "{}: {error}",
                path.display()
            )))),
        })
        .transpose()?
        .flatten();
    let system = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()));
    select_language(&LanguageSources {
        explicit: explicit.map(str::to_owned),
        environment: std::env::var("POP_LANGUAGE")
            .ok()
            .filter(|value| !value.is_empty()),
        config,
        system,
        config_path,
    })
}

#[must_use]
pub fn user_config_path() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(root).join("pop/config.toml"));
    }
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".config/pop/config.toml"))
}

#[derive(Clone, Debug, Deserialize)]
struct RawFragment {
    messages: BTreeMap<String, Message>,
}

#[derive(Clone, Debug, Deserialize)]
struct RawMetadata {
    tag: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Message {
    text: String,
    #[serde(default)]
    arguments: Vec<String>,
    #[serde(default)]
    kinds: Vec<String>,
}

impl Message {
    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    #[must_use]
    pub fn kinds(&self) -> &[String] {
        &self.kinds
    }
}

#[derive(Clone, Debug)]
pub struct Catalog {
    tag: String,
    messages: BTreeMap<String, Message>,
}

impl Catalog {
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    #[must_use]
    pub const fn messages(&self) -> &BTreeMap<String, Message> {
        &self.messages
    }
}

static CATALOGS: OnceLock<Result<BTreeMap<Language, Catalog>, LocalizationError>> = OnceLock::new();

/// Loads and validates all embedded official localization catalogs.
///
/// # Errors
///
/// Returns an error when any embedded manifest, fragment, key, placeholder, or
/// typed argument schema violates the canonical English catalog.
pub fn official_catalogs() -> Result<&'static BTreeMap<Language, Catalog>, LocalizationError> {
    CATALOGS
        .get_or_init(load_catalogs)
        .as_ref()
        .map_err(Clone::clone)
}

fn load_catalogs() -> Result<BTreeMap<Language, Catalog>, LocalizationError> {
    let mut catalogs: BTreeMap<Language, Catalog> = BTreeMap::new();
    for (language, source) in [
        (Language::English, &ENGLISH),
        (Language::SimplifiedChinese, &SIMPLIFIED_CHINESE),
        (Language::Japanese, &JAPANESE),
        (Language::PortugueseBrazil, &PORTUGUESE_BRAZIL),
        (Language::Spanish, &SPANISH),
    ] {
        let metadata: RawMetadata = toml::from_str(source.manifest).map_err(|error| {
            LocalizationError::InvalidCatalog(format!("{}/manifest.toml: {error}", language.tag()))
        })?;
        if metadata.tag != language.tag() {
            return Err(LocalizationError::InvalidCatalog(format!(
                "{} declares tag {}",
                language.tag(),
                metadata.tag
            )));
        }
        let mut messages = BTreeMap::new();
        for (path, fragment_source) in source.fragments {
            let fragment: RawFragment = toml::from_str(fragment_source).map_err(|error| {
                LocalizationError::InvalidCatalog(format!("{}/{path}: {error}", language.tag()))
            })?;
            for (key, message) in fragment.messages {
                if messages.insert(key.clone(), message).is_some() {
                    return Err(LocalizationError::InvalidCatalog(format!(
                        "{} message {key} is declared in more than one fragment",
                        language.tag()
                    )));
                }
            }
        }
        if language == Language::English {
            validate_messages(language, &messages)?;
        } else {
            let canonical = catalogs.get(&Language::English).ok_or_else(|| {
                LocalizationError::InvalidCatalog("English must load first".to_owned())
            })?;
            if canonical.messages.keys().ne(messages.keys()) {
                return Err(LocalizationError::InvalidCatalog(format!(
                    "{} does not have exact English key parity",
                    language.tag()
                )));
            }
            for (key, message) in &mut messages {
                let expected = &canonical.messages[key];
                if !message.arguments.is_empty() || !message.kinds.is_empty() {
                    return Err(LocalizationError::InvalidCatalog(format!(
                        "{} message {key} repeats the canonical argument schema",
                        language.tag()
                    )));
                }
                message.arguments.clone_from(&expected.arguments);
                message.kinds.clone_from(&expected.kinds);
            }
            validate_messages(language, &messages)?;
        }
        catalogs.insert(
            language,
            Catalog {
                tag: metadata.tag,
                messages,
            },
        );
    }

    let canonical = &catalogs[&Language::English].messages;
    for language in Language::ALL.into_iter().skip(1) {
        let translated = &catalogs[&language].messages;
        if canonical.keys().ne(translated.keys()) {
            return Err(LocalizationError::InvalidCatalog(format!(
                "{} does not have exact English key parity",
                language.tag()
            )));
        }
        for (key, message) in translated {
            let expected = &canonical[key];
            if message.arguments != expected.arguments || message.kinds != expected.kinds {
                return Err(LocalizationError::InvalidCatalog(format!(
                    "{} message {key} does not match English arguments",
                    language.tag()
                )));
            }
        }
    }
    Ok(catalogs)
}

fn validate_messages(
    language: Language,
    messages: &BTreeMap<String, Message>,
) -> Result<(), LocalizationError> {
    for (key, message) in messages {
        if message.arguments.len() != message.kinds.len() {
            return Err(LocalizationError::InvalidCatalog(format!(
                "{} message {key} has mismatched argument and kind counts",
                language.tag()
            )));
        }
        if message
            .text
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        {
            return Err(LocalizationError::InvalidCatalog(format!(
                "{} message {key} contains a control character",
                language.tag()
            )));
        }
        let placeholders = placeholders(&message.text).map_err(|reason| {
            LocalizationError::InvalidCatalog(format!("{} message {key}: {reason}", language.tag()))
        })?;
        let declared = message.arguments.iter().cloned().collect::<BTreeSet<_>>();
        if placeholders != declared || declared.len() != message.arguments.len() {
            return Err(LocalizationError::InvalidCatalog(format!(
                "{} message {key} placeholder set differs from its arguments",
                language.tag()
            )));
        }
    }
    Ok(())
}

fn placeholders(text: &str) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        let end = rest
            .find('}')
            .ok_or_else(|| "unclosed placeholder".to_owned())?;
        let name = &rest[..end];
        if name.is_empty()
            || !name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(format!("invalid placeholder {{{name}}}"));
        }
        names.insert(name.to_owned());
        rest = &rest[end + 1..];
    }
    if rest.contains('}') {
        return Err("unmatched closing brace".to_owned());
    }
    Ok(names)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArgumentKind {
    Character,
    External,
    Identifier,
    SyntaxExpectation,
    Text,
    Token,
    Unsigned,
}

impl ArgumentKind {
    const fn name(self) -> &'static str {
        match self {
            Self::Character => "Character",
            Self::External => "External",
            Self::Identifier => "Identifier",
            Self::SyntaxExpectation => "SyntaxExpectation",
            Self::Text => "Text",
            Self::Token => "Token",
            Self::Unsigned => "Unsigned",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Argument {
    name: String,
    kind: ArgumentKind,
    value: String,
}

impl Argument {
    #[must_use]
    pub fn text(name: impl Into<String>, value: impl fmt::Display) -> Self {
        Self::new(name, ArgumentKind::Text, value)
    }

    #[must_use]
    pub fn external(name: impl Into<String>, value: impl fmt::Display) -> Self {
        Self::new(name, ArgumentKind::External, value)
    }

    #[must_use]
    pub fn unsigned(name: impl Into<String>, value: u64) -> Self {
        Self::new(name, ArgumentKind::Unsigned, value)
    }

    #[must_use]
    pub fn character(name: impl Into<String>, value: char) -> Self {
        Self::new(name, ArgumentKind::Character, value)
    }

    fn new(name: impl Into<String>, kind: ArgumentKind, value: impl fmt::Display) -> Self {
        Self {
            name: name.into(),
            kind,
            value: escape_control_characters(&value.to_string()),
        }
    }

    fn diagnostic(name: &str, argument: &DiagnosticArgument) -> Self {
        match argument {
            DiagnosticArgument::Character(value) => Self::new(name, ArgumentKind::Character, value),
            DiagnosticArgument::Identifier(value) => {
                Self::new(name, ArgumentKind::Identifier, value)
            }
            DiagnosticArgument::Unsigned(value) => Self::new(name, ArgumentKind::Unsigned, value),
            DiagnosticArgument::SyntaxExpectation(value) => {
                Self::new(name, ArgumentKind::SyntaxExpectation, value)
            }
            DiagnosticArgument::Token(value) => Self::new(name, ArgumentKind::Token, value),
        }
    }
}

fn escape_control_characters(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\t') {
                format!("\\u{{{:x}}}", u32::from(character))
                    .chars()
                    .collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderContext {
    language: Language,
}

impl RenderContext {
    #[must_use]
    pub const fn new(language: Language) -> Self {
        Self { language }
    }

    #[must_use]
    pub const fn language(self) -> Language {
        self.language
    }

    /// Renders one statically keyed tool message with typed named arguments.
    ///
    /// # Errors
    ///
    /// Returns an error when the key is unknown or the supplied argument names,
    /// kinds, or count do not match the catalog schema.
    pub fn message(self, key: &str, arguments: &[Argument]) -> Result<String, LocalizationError> {
        let catalogs = official_catalogs()?;
        let message = catalogs[&self.language]
            .messages
            .get(key)
            .ok_or_else(|| LocalizationError::UnknownMessage(key.to_owned()))?;
        if arguments.len() != message.arguments.len() {
            return Err(LocalizationError::InvalidArguments(format!(
                "message {key} expects {} arguments, found {}",
                message.arguments.len(),
                arguments.len()
            )));
        }
        let supplied = arguments
            .iter()
            .map(|argument| (argument.name.as_str(), argument))
            .collect::<BTreeMap<_, _>>();
        for (name, expected_kind) in message.arguments.iter().zip(&message.kinds) {
            let argument = supplied.get(name.as_str()).ok_or_else(|| {
                LocalizationError::InvalidArguments(format!(
                    "message {key} is missing argument {name}"
                ))
            })?;
            if argument.kind.name() != expected_kind && argument.kind != ArgumentKind::Text {
                return Err(LocalizationError::InvalidArguments(format!(
                    "message {key} argument {name} expects {expected_kind}, found {}",
                    argument.kind.name()
                )));
            }
        }
        let mut rendered = String::new();
        let mut rest = message.text.as_str();
        while let Some(start) = rest.find('{') {
            rendered.push_str(&rest[..start]);
            rest = &rest[start + 1..];
            if let Some(end) = rest.find('}') {
                let name = &rest[..end];
                if let Some(argument) = supplied.get(name) {
                    rendered.push_str(&argument.value);
                } else {
                    rendered.push('{');
                    rendered.push_str(name);
                    rendered.push('}');
                }
                rest = &rest[end + 1..];
            } else {
                rendered.push('{');
            }
        }
        rendered.push_str(rest);
        Ok(rendered)
    }

    /// Renders a complete structured diagnostic for human CLI presentation.
    ///
    /// # Errors
    ///
    /// Returns an error when a diagnostic, label, note, or fix references an
    /// unknown message or supplies arguments that violate its schema.
    pub fn diagnostic(self, diagnostic: &Diagnostic) -> Result<String, LocalizationError> {
        let message =
            self.diagnostic_message(diagnostic.message_key().as_str(), diagnostic.arguments())?;
        let severity_key = match diagnostic.severity() {
            DiagnosticSeverity::Error => "ui.error",
            DiagnosticSeverity::Warning => "ui.warning",
            DiagnosticSeverity::Information => "ui.information",
            DiagnosticSeverity::Hint => "ui.hint",
        };
        let severity = self.message(severity_key, &[])?;
        let range = diagnostic.primary_span().range();
        let mut output = format!(
            "{severity}[{}]: {message}\n  --> file#{}:{}..{}\n",
            diagnostic.code(),
            diagnostic.primary_span().file().raw(),
            range.start().to_u32(),
            range.end().to_u32()
        );
        for label in diagnostic.labels() {
            let label_text =
                self.diagnostic_message(label.message_key().as_str(), label.arguments())?;
            let label_name = self.message("ui.label", &[])?;
            let range = label.span().range();
            let _ = writeln!(
                output,
                "  {label_name} file#{}:{}..{}: {label_text}",
                label.span().file().raw(),
                range.start().to_u32(),
                range.end().to_u32()
            );
        }
        for note in diagnostic.notes() {
            let note_text =
                self.diagnostic_message(note.message_key().as_str(), note.arguments())?;
            let _ = writeln!(output, "  {}: {note_text}", self.message("ui.note", &[])?);
        }
        for fix in diagnostic.fixes() {
            let title = self.diagnostic_message(fix.title_key().as_str(), &[])?;
            let prefix = if fix.is_safe() {
                self.message("ui.help", &[])?
            } else {
                self.message("ui.suggestion", &[])?
            };
            let _ = writeln!(output, "  {prefix}: {title}");
        }
        Ok(output)
    }

    /// Renders only a diagnostic's localized message for protocol adapters.
    ///
    /// # Errors
    ///
    /// Returns an error when the message key is unknown or its typed argument
    /// schema does not match the diagnostic.
    pub fn diagnostic_message_only(
        self,
        diagnostic: &Diagnostic,
    ) -> Result<String, LocalizationError> {
        self.diagnostic_message(diagnostic.message_key().as_str(), diagnostic.arguments())
    }

    /// Renders one compiler diagnostic component from its typed catalog key.
    ///
    /// # Errors
    ///
    /// Returns an error when the key or typed arguments do not match the
    /// immutable catalog schema.
    pub fn diagnostic_message(
        self,
        key: &str,
        arguments: &[DiagnosticArgument],
    ) -> Result<String, LocalizationError> {
        let catalogs = official_catalogs()?;
        let definition = catalogs[&self.language]
            .messages
            .get(key)
            .ok_or_else(|| LocalizationError::UnknownMessage(key.to_owned()))?;
        if definition.arguments.len() != arguments.len() {
            return Err(LocalizationError::InvalidArguments(format!(
                "diagnostic message {key} expects {} arguments, found {}",
                definition.arguments.len(),
                arguments.len()
            )));
        }
        let bound = definition
            .arguments
            .iter()
            .zip(arguments)
            .map(|(name, argument)| Argument::diagnostic(name, argument))
            .collect::<Vec<_>>();
        self.message(key, &bound)
    }
}
