use pop_foundation::{
    Diagnostic, DiagnosticArgument, DiagnosticCategory, DiagnosticCode, DiagnosticSeverity,
    MessageKey, SourceSpan,
};

#[must_use]
pub fn invalid_character(span: SourceSpan, character: char) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP0001"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Syntax,
        MessageKey::new("syntax.invalidCharacter"),
        vec![DiagnosticArgument::Character(character)],
        span,
    )
}

#[must_use]
pub fn unterminated_string(span: SourceSpan) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP0006"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Syntax,
        MessageKey::new("syntax.unterminatedString"),
        Vec::new(),
        span,
    )
}

#[must_use]
pub fn invalid_string_escape(span: SourceSpan) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP0007"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Syntax,
        MessageKey::new("syntax.invalidStringEscape"),
        Vec::new(),
        span,
    )
}
