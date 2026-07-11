use pop_foundation::{
    Diagnostic, DiagnosticArgument, DiagnosticCategory, DiagnosticCode, DiagnosticLabel,
    DiagnosticSeverity, MessageKey, SourceSpan,
};

#[must_use]
pub fn invalid_declaration(span: SourceSpan) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP1000"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Resolution,
        MessageKey::new("resolution.invalidDeclaration"),
        Vec::new(),
        span,
    )
}

#[must_use]
pub fn duplicate_declaration(
    span: SourceSpan,
    name: impl Into<String>,
    original: SourceSpan,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP1001"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Resolution,
        MessageKey::new("resolution.duplicateDeclaration"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
    .with_label(DiagnosticLabel::new(
        original,
        MessageKey::new("resolution.originalDeclaration"),
        Vec::new(),
    ))
}

#[must_use]
pub fn unknown_name(span: SourceSpan, name: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP1002"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Resolution,
        MessageKey::new("resolution.unknownName"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
}

#[must_use]
pub fn ambiguous_name(
    span: SourceSpan,
    name: impl Into<String>,
    candidates: impl IntoIterator<Item = SourceSpan>,
) -> Diagnostic {
    candidates.into_iter().fold(
        Diagnostic::new(
            DiagnosticCode::new("POP1003"),
            DiagnosticSeverity::Error,
            DiagnosticCategory::Resolution,
            MessageKey::new("resolution.ambiguousName"),
            vec![DiagnosticArgument::Identifier(name.into())],
            span,
        ),
        |diagnostic, candidate| {
            diagnostic.with_label(DiagnosticLabel::new(
                candidate,
                MessageKey::new("resolution.candidate"),
                Vec::new(),
            ))
        },
    )
}

#[must_use]
pub fn inaccessible_name(
    span: SourceSpan,
    name: impl Into<String>,
    declaration: SourceSpan,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP1004"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Resolution,
        MessageKey::new("resolution.inaccessibleName"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
    .with_label(DiagnosticLabel::new(
        declaration,
        MessageKey::new("resolution.declaration"),
        Vec::new(),
    ))
}
