use pop_foundation::{
    Diagnostic, DiagnosticArgument, DiagnosticCategory, DiagnosticCode, DiagnosticSeverity,
    MessageKey, SourceSpan,
};

#[must_use]
pub fn unsafe_xml(span: SourceSpan) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP6400"),
        DiagnosticSeverity::Warning,
        DiagnosticCategory::Style,
        MessageKey::new("documentation.unsafeXml"),
        Vec::new(),
        span,
    )
}

#[must_use]
pub fn invalid_error_tag(span: SourceSpan, error_type: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP6402"),
        DiagnosticSeverity::Warning,
        DiagnosticCategory::Style,
        MessageKey::new("documentation.invalidErrorTag"),
        vec![DiagnosticArgument::Identifier(error_type.into())],
        span,
    )
}

#[must_use]
pub fn missing_error_case(span: SourceSpan, error_case: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP6403"),
        DiagnosticSeverity::Warning,
        DiagnosticCategory::Style,
        MessageKey::new("documentation.missingErrorCase"),
        vec![DiagnosticArgument::Identifier(error_case.into())],
        span,
    )
}

#[must_use]
pub fn missing_summary(span: SourceSpan, declaration: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP6404"),
        DiagnosticSeverity::Warning,
        DiagnosticCategory::Style,
        MessageKey::new("documentation.missingSummary"),
        vec![DiagnosticArgument::Identifier(declaration.into())],
        span,
    )
}

#[must_use]
pub fn duplicate_summary(span: SourceSpan, declaration: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP6405"),
        DiagnosticSeverity::Warning,
        DiagnosticCategory::Style,
        MessageKey::new("documentation.duplicateSummary"),
        vec![DiagnosticArgument::Identifier(declaration.into())],
        span,
    )
}

#[must_use]
pub fn invalid_inheritance(span: SourceSpan, source: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP6406"),
        DiagnosticSeverity::Warning,
        DiagnosticCategory::Style,
        MessageKey::new("documentation.invalidInheritance"),
        vec![DiagnosticArgument::Identifier(source.into())],
        span,
    )
}

#[must_use]
pub fn inheritance_cycle(span: SourceSpan, declaration: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP6407"),
        DiagnosticSeverity::Warning,
        DiagnosticCategory::Style,
        MessageKey::new("documentation.inheritanceCycle"),
        vec![DiagnosticArgument::Identifier(declaration.into())],
        span,
    )
}

#[must_use]
pub fn invalid_returns(span: SourceSpan, expectation: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP6408"),
        DiagnosticSeverity::Warning,
        DiagnosticCategory::Style,
        MessageKey::new("documentation.invalidReturns"),
        vec![DiagnosticArgument::Identifier(expectation.into())],
        span,
    )
}

#[must_use]
pub fn malformed_xml(span: SourceSpan) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP6401"),
        DiagnosticSeverity::Warning,
        DiagnosticCategory::Style,
        MessageKey::new("documentation.malformedXml"),
        Vec::new(),
        span,
    )
}
