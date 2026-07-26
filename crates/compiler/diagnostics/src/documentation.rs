use pop_foundation::{
    Diagnostic, DiagnosticArgument, DiagnosticCategory, DiagnosticCode, DiagnosticSeverity,
    MessageKey, SourceSpan, SuppressionKey, WarningWave,
};

fn warning(
    code: &'static str,
    message: &'static str,
    arguments: Vec<DiagnosticArgument>,
    span: SourceSpan,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new(code),
        DiagnosticSeverity::Warning,
        DiagnosticCategory::Style,
        MessageKey::new(message),
        arguments,
        span,
    )
    .with_warning_wave(WarningWave::new(1))
    .with_suppression_key(SuppressionKey::new(code))
}

#[must_use]
pub fn unsafe_xml(span: SourceSpan) -> Diagnostic {
    warning("POP6400", "documentation.unsafeXml", Vec::new(), span)
}

#[must_use]
pub fn invalid_error_tag(span: SourceSpan, error_type: impl Into<String>) -> Diagnostic {
    warning(
        "POP6402",
        "documentation.invalidErrorTag",
        vec![DiagnosticArgument::Identifier(error_type.into())],
        span,
    )
}

#[must_use]
pub fn missing_error_case(span: SourceSpan, error_case: impl Into<String>) -> Diagnostic {
    warning(
        "POP6403",
        "documentation.missingErrorCase",
        vec![DiagnosticArgument::Identifier(error_case.into())],
        span,
    )
}

#[must_use]
pub fn missing_summary(span: SourceSpan, declaration: impl Into<String>) -> Diagnostic {
    warning(
        "POP6404",
        "documentation.missingSummary",
        vec![DiagnosticArgument::Identifier(declaration.into())],
        span,
    )
}

#[must_use]
pub fn duplicate_summary(span: SourceSpan, declaration: impl Into<String>) -> Diagnostic {
    warning(
        "POP6405",
        "documentation.duplicateSummary",
        vec![DiagnosticArgument::Identifier(declaration.into())],
        span,
    )
}

#[must_use]
pub fn invalid_inheritance(span: SourceSpan, source: impl Into<String>) -> Diagnostic {
    warning(
        "POP6406",
        "documentation.invalidInheritance",
        vec![DiagnosticArgument::Identifier(source.into())],
        span,
    )
}

#[must_use]
pub fn inheritance_cycle(span: SourceSpan, declaration: impl Into<String>) -> Diagnostic {
    warning(
        "POP6407",
        "documentation.inheritanceCycle",
        vec![DiagnosticArgument::Identifier(declaration.into())],
        span,
    )
}

#[must_use]
pub fn invalid_returns(span: SourceSpan, expectation: impl Into<String>) -> Diagnostic {
    warning(
        "POP6408",
        "documentation.invalidReturns",
        vec![DiagnosticArgument::Identifier(expectation.into())],
        span,
    )
}

#[must_use]
pub fn malformed_xml(span: SourceSpan) -> Diagnostic {
    warning("POP6401", "documentation.malformedXml", Vec::new(), span)
}
