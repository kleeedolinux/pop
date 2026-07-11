use pop_foundation::{
    Diagnostic, DiagnosticCategory, DiagnosticCode, DiagnosticSeverity, MessageKey, SourceSpan,
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
