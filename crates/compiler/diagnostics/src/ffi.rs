use pop_foundation::{
    Diagnostic, DiagnosticArgument, DiagnosticCategory, DiagnosticCode, DiagnosticSeverity,
    MessageKey, SourceSpan,
};

#[must_use]
pub fn invalid_foreign_contract(span: SourceSpan, reason: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP5000"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::RuntimeSafety,
        MessageKey::new("ffi.invalidForeignContract"),
        vec![DiagnosticArgument::Identifier(reason.into())],
        span,
    )
}
