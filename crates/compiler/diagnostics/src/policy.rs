use std::num::NonZeroUsize;

use pop_foundation::{Diagnostic, DiagnosticSeverity};

use crate::catalog;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum WarningGroup {
    Correctness,
    Nullability,
    Concurrency,
    Unsafe,
    Performance,
    Allocation,
    ApiDesign,
    Documentation,
    Style,
    Compatibility,
    Deprecated,
    Unused,
}

impl WarningGroup {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Correctness => "Correctness",
            Self::Nullability => "Nullability",
            Self::Concurrency => "Concurrency",
            Self::Unsafe => "Unsafe",
            Self::Performance => "Performance",
            Self::Allocation => "Allocation",
            Self::ApiDesign => "ApiDesign",
            Self::Documentation => "Documentation",
            Self::Style => "Style",
            Self::Compatibility => "Compatibility",
            Self::Deprecated => "Deprecated",
            Self::Unused => "Unused",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Correctness" => Some(Self::Correctness),
            "Nullability" => Some(Self::Nullability),
            "Concurrency" => Some(Self::Concurrency),
            "Unsafe" => Some(Self::Unsafe),
            "Performance" => Some(Self::Performance),
            "Allocation" => Some(Self::Allocation),
            "ApiDesign" => Some(Self::ApiDesign),
            "Documentation" => Some(Self::Documentation),
            "Style" => Some(Self::Style),
            "Compatibility" => Some(Self::Compatibility),
            "Deprecated" => Some(Self::Deprecated),
            "Unused" => Some(Self::Unused),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticSelector {
    All,
    Group(WarningGroup),
    Code(String),
}

impl DiagnosticSelector {
    /// Parses `*`, one stable warning-group name, or one exact `POP####` code.
    ///
    /// # Errors
    ///
    /// Returns the supplied value when it is not a supported selector.
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value == "*" {
            return Ok(Self::All);
        }
        if let Some(group) = WarningGroup::parse(&value) {
            return Ok(Self::Group(group));
        }
        Self::code(value)
    }

    /// Creates an exact built-in diagnostic-code selector.
    ///
    /// # Errors
    ///
    /// Returns the supplied value when it is not `POP` followed by four digits.
    pub fn code(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        let bytes = value.as_bytes();
        if bytes.len() == 7
            && bytes.starts_with(b"POP")
            && bytes[3..].iter().all(u8::is_ascii_digit)
        {
            Ok(Self::Code(value))
        } else {
            Err(value)
        }
    }

    fn matches(&self, diagnostic: &Diagnostic, group: Option<WarningGroup>) -> bool {
        match self {
            Self::All => true,
            Self::Group(expected) => group == Some(*expected),
            Self::Code(expected) => expected == diagnostic.code().as_str(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticPolicy {
    warning_wave: u32,
    warnings_as_errors: Vec<DiagnosticSelector>,
    disabled_warnings: Vec<DiagnosticSelector>,
}

impl DiagnosticPolicy {
    #[must_use]
    pub const fn new(warning_wave: u32) -> Self {
        Self {
            warning_wave,
            warnings_as_errors: Vec::new(),
            disabled_warnings: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_warnings_as_errors(
        mut self,
        selectors: impl IntoIterator<Item = DiagnosticSelector>,
    ) -> Self {
        self.warnings_as_errors.extend(selectors);
        self
    }

    #[must_use]
    pub fn with_disabled_warnings(
        mut self,
        selectors: impl IntoIterator<Item = DiagnosticSelector>,
    ) -> Self {
        self.disabled_warnings.extend(selectors);
        self
    }

    #[must_use]
    pub fn evaluate(&self, diagnostic: &Diagnostic) -> DiagnosticDisposition {
        if diagnostic.severity() != DiagnosticSeverity::Warning {
            return DiagnosticDisposition {
                enabled: true,
                blocks_artifact: diagnostic.severity() == DiagnosticSeverity::Error,
                promoted: false,
            };
        }

        let metadata = catalog()
            .ok()
            .into_iter()
            .flatten()
            .find(|entry| entry.code() == diagnostic.code());
        let wave = diagnostic.warning_wave().map_or_else(
            || metadata.and_then(crate::CatalogEntry::warning_wave),
            |wave| Some(wave.value()),
        );
        let group = metadata.and_then(crate::CatalogEntry::warning_group);
        let enabled_by_wave = wave.is_none_or(|wave| wave <= self.warning_wave);
        let disabled = self
            .disabled_warnings
            .iter()
            .any(|selector| selector.matches(diagnostic, group));
        let enabled = enabled_by_wave && !disabled;
        let promoted = enabled
            && self
                .warnings_as_errors
                .iter()
                .any(|selector| selector.matches(diagnostic, group));
        DiagnosticDisposition {
            enabled,
            blocks_artifact: promoted,
            promoted,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticDisposition {
    enabled: bool,
    blocks_artifact: bool,
    promoted: bool,
}

impl DiagnosticDisposition {
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn blocks_artifact(self) -> bool {
        self.blocks_artifact
    }

    #[must_use]
    pub const fn is_promoted(self) -> bool {
        self.promoted
    }
}

#[derive(Debug)]
pub struct DiagnosticReport<'diagnostics> {
    diagnostics: Vec<&'diagnostics Diagnostic>,
    error_count: usize,
    warning_count: usize,
    omitted_error_count: usize,
    blocks_artifact: bool,
}

impl<'diagnostics> DiagnosticReport<'diagnostics> {
    #[must_use]
    pub fn new(
        diagnostics: &'diagnostics [Diagnostic],
        policy: &DiagnosticPolicy,
        maximum_errors: NonZeroUsize,
    ) -> Self {
        let mut visible = Vec::new();
        let mut error_count = 0;
        let mut warning_count = 0;
        let mut omitted_error_count = 0;
        let mut visible_blockers = 0;
        let mut blocks_artifact = false;
        for diagnostic in diagnostics {
            let disposition = policy.evaluate(diagnostic);
            if !disposition.is_enabled() {
                continue;
            }
            if diagnostic.severity() == DiagnosticSeverity::Warning {
                warning_count += 1;
            }
            if disposition.blocks_artifact() {
                error_count += 1;
                blocks_artifact = true;
                if visible_blockers == maximum_errors.get() {
                    omitted_error_count += 1;
                    continue;
                }
                visible_blockers += 1;
            }
            visible.push(diagnostic);
        }
        Self {
            diagnostics: visible,
            error_count,
            warning_count,
            omitted_error_count,
            blocks_artifact,
        }
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[&'diagnostics Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub const fn error_count(&self) -> usize {
        self.error_count
    }

    #[must_use]
    pub const fn warning_count(&self) -> usize {
        self.warning_count
    }

    #[must_use]
    pub const fn omitted_error_count(&self) -> usize {
        self.omitted_error_count
    }

    #[must_use]
    pub const fn reached_error_limit(&self) -> bool {
        self.omitted_error_count != 0
    }

    #[must_use]
    pub const fn blocks_artifact(&self) -> bool {
        self.blocks_artifact
    }
}
