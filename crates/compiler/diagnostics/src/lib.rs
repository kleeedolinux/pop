//! Catalog-backed diagnostic constructors.

use std::error::Error;
use std::fmt;

use pop_foundation::{DiagnosticCategory, DiagnosticCode, DiagnosticSeverity};

pub mod compile_time;
pub mod documentation;
pub mod lexing;
pub mod resolution;
pub mod syntax;
pub mod types;

const CATALOG: &str = include_str!("../catalog.tsv");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CatalogEntry {
    code: DiagnosticCode,
    severity: DiagnosticSeverity,
    category: DiagnosticCategory,
    warning_wave: Option<u32>,
    message_key: &'static str,
    help_key: Option<&'static str>,
    argument_schema: &'static str,
    suppressible: bool,
    documentation_url: &'static str,
    owner: &'static str,
    quick_fix_providers: &'static str,
    edition_introduced: &'static str,
    edition_deprecated: Option<&'static str>,
}

impl CatalogEntry {
    #[must_use]
    pub const fn code(self) -> DiagnosticCode {
        self.code
    }

    #[must_use]
    pub const fn severity(self) -> DiagnosticSeverity {
        self.severity
    }

    #[must_use]
    pub const fn category(self) -> DiagnosticCategory {
        self.category
    }

    #[must_use]
    pub const fn warning_wave(self) -> Option<u32> {
        self.warning_wave
    }

    #[must_use]
    pub const fn message_key(self) -> &'static str {
        self.message_key
    }

    #[must_use]
    pub const fn owner(self) -> &'static str {
        self.owner
    }

    #[must_use]
    pub const fn argument_schema(self) -> &'static str {
        self.argument_schema
    }

    #[must_use]
    pub const fn is_suppressible(self) -> bool {
        self.suppressible
    }

    #[must_use]
    pub const fn documentation_url(self) -> &'static str {
        self.documentation_url
    }

    #[must_use]
    pub const fn help_key(self) -> Option<&'static str> {
        self.help_key
    }

    #[must_use]
    pub const fn quick_fix_providers(self) -> &'static str {
        self.quick_fix_providers
    }

    #[must_use]
    pub const fn edition_introduced(self) -> &'static str {
        self.edition_introduced
    }

    #[must_use]
    pub const fn edition_deprecated(self) -> Option<&'static str> {
        self.edition_deprecated
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogError {
    line: usize,
    reason: &'static str,
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid diagnostic catalog line {}: {}",
            self.line, self.reason
        )
    }
}

impl Error for CatalogError {}

/// Parses the embedded machine-readable diagnostic catalog.
///
/// # Errors
///
/// Returns [`CatalogError`] when an entry does not match the catalog schema.
pub fn catalog() -> Result<Vec<CatalogEntry>, CatalogError> {
    CATALOG
        .lines()
        .enumerate()
        .skip(1)
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| parse_entry(index + 1, line))
        .collect()
}

fn parse_entry(line_number: usize, line: &'static str) -> Result<CatalogEntry, CatalogError> {
    let mut fields = line.split('\t');
    let code = fields.next().ok_or(CatalogError {
        line: line_number,
        reason: "missing code",
    })?;
    let severity = match fields.next() {
        Some("Error") => DiagnosticSeverity::Error,
        Some("Warning") => DiagnosticSeverity::Warning,
        Some("Information") => DiagnosticSeverity::Information,
        Some("Hint") => DiagnosticSeverity::Hint,
        _ => {
            return Err(CatalogError {
                line: line_number,
                reason: "invalid severity",
            });
        }
    };
    let category = match fields.next() {
        Some("Syntax") => DiagnosticCategory::Syntax,
        Some("Resolution") => DiagnosticCategory::Resolution,
        Some("Type") => DiagnosticCategory::Type,
        Some("CompileTime") => DiagnosticCategory::CompileTime,
        Some("Style") => DiagnosticCategory::Style,
        _ => {
            return Err(CatalogError {
                line: line_number,
                reason: "invalid category",
            });
        }
    };
    let warning_wave = optional_u32(
        next_field(&mut fields, line_number, "warning wave")?,
        line_number,
    )?;
    let message_key = fields.next().ok_or(CatalogError {
        line: line_number,
        reason: "missing message key",
    })?;
    let help_key = optional_field(next_field(&mut fields, line_number, "help key")?);
    let argument_schema = next_field(&mut fields, line_number, "argument schema")?;
    let suppressible = match next_field(&mut fields, line_number, "suppressibility")? {
        "true" => true,
        "false" => false,
        _ => {
            return Err(CatalogError {
                line: line_number,
                reason: "invalid suppressibility",
            });
        }
    };
    let documentation_url = next_field(&mut fields, line_number, "documentation URL")?;
    let owner = fields.next().ok_or(CatalogError {
        line: line_number,
        reason: "missing owner",
    })?;
    let quick_fix_providers = next_field(&mut fields, line_number, "quick-fix providers")?;
    let edition_introduced = next_field(&mut fields, line_number, "introduced edition")?;
    let edition_deprecated =
        optional_field(next_field(&mut fields, line_number, "deprecated edition")?);
    if fields.next().is_some() {
        return Err(CatalogError {
            line: line_number,
            reason: "unexpected field",
        });
    }

    Ok(CatalogEntry {
        code: DiagnosticCode::new(code),
        severity,
        category,
        warning_wave,
        message_key,
        help_key,
        argument_schema,
        suppressible,
        documentation_url,
        owner,
        quick_fix_providers,
        edition_introduced,
        edition_deprecated,
    })
}

fn next_field(
    fields: &mut impl Iterator<Item = &'static str>,
    line: usize,
    _name: &'static str,
) -> Result<&'static str, CatalogError> {
    fields.next().ok_or(CatalogError {
        line,
        reason: "missing catalog field",
    })
}

fn optional_field(value: &'static str) -> Option<&'static str> {
    (value != "-").then_some(value)
}

fn optional_u32(value: &'static str, line: usize) -> Result<Option<u32>, CatalogError> {
    if value == "-" {
        return Ok(None);
    }
    value.parse().map(Some).map_err(|_| CatalogError {
        line,
        reason: "invalid numeric catalog field",
    })
}
