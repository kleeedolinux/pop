use pop_foundation::{
    Diagnostic, DiagnosticArgument, DiagnosticCategory, DiagnosticCode, DiagnosticOrigin,
    DiagnosticSeverity, MessageKey, SourceSpan,
};

fn constant_error(
    code: &'static str,
    message_key: &'static str,
    span: SourceSpan,
    context: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new(code),
        DiagnosticSeverity::Error,
        DiagnosticCategory::CompileTime,
        MessageKey::new(message_key),
        vec![DiagnosticArgument::Identifier(context.into())],
        span,
    )
}

#[must_use]
pub fn ineligible_constant_expression(span: SourceSpan, context: impl Into<String>) -> Diagnostic {
    constant_error(
        "POP4001",
        "compileTime.ineligibleConstantExpression",
        span,
        context,
    )
}

#[must_use]
pub fn constant_integer_overflow(span: SourceSpan, context: impl Into<String>) -> Diagnostic {
    constant_error(
        "POP4002",
        "compileTime.constantIntegerOverflow",
        span,
        context,
    )
}

#[must_use]
pub fn constant_division_by_zero(span: SourceSpan, context: impl Into<String>) -> Diagnostic {
    constant_error(
        "POP4003",
        "compileTime.constantDivisionByZero",
        span,
        context,
    )
}

fn execution_error(
    code: &'static str,
    message_key: &'static str,
    span: SourceSpan,
    arguments: Vec<DiagnosticArgument>,
    origins: impl IntoIterator<Item = DiagnosticOrigin>,
) -> Diagnostic {
    origins.into_iter().fold(
        Diagnostic::new(
            DiagnosticCode::new(code),
            DiagnosticSeverity::Error,
            DiagnosticCategory::CompileTime,
            MessageKey::new(message_key),
            arguments,
            span,
        ),
        Diagnostic::with_origin,
    )
}

#[must_use]
pub fn function_not_eligible(
    span: SourceSpan,
    function: impl Into<String>,
    origins: impl IntoIterator<Item = DiagnosticOrigin>,
) -> Diagnostic {
    execution_error(
        "POP4004",
        "compileTime.functionNotEligible",
        span,
        vec![DiagnosticArgument::Identifier(function.into())],
        origins,
    )
}

#[must_use]
pub fn forbidden_effect(
    span: SourceSpan,
    function: impl Into<String>,
    effect: impl Into<String>,
    origins: impl IntoIterator<Item = DiagnosticOrigin>,
) -> Diagnostic {
    execution_error(
        "POP4005",
        "compileTime.forbiddenEffect",
        span,
        vec![
            DiagnosticArgument::Identifier(function.into()),
            DiagnosticArgument::Identifier(effect.into()),
        ],
        origins,
    )
}

#[must_use]
pub fn cycle(
    span: SourceSpan,
    cycle: impl Into<String>,
    origins: impl IntoIterator<Item = DiagnosticOrigin>,
) -> Diagnostic {
    execution_error(
        "POP4006",
        "compileTime.cycle",
        span,
        vec![DiagnosticArgument::Identifier(cycle.into())],
        origins,
    )
}

#[must_use]
pub fn resource_limit(
    span: SourceSpan,
    resource: impl Into<String>,
    limit: u64,
    origins: impl IntoIterator<Item = DiagnosticOrigin>,
) -> Diagnostic {
    execution_error(
        "POP4007",
        "compileTime.resourceLimit",
        span,
        vec![
            DiagnosticArgument::Identifier(resource.into()),
            DiagnosticArgument::Unsigned(limit),
        ],
        origins,
    )
}

#[must_use]
pub fn attribute_validator_rejected(
    span: SourceSpan,
    attribute: impl Into<String>,
    origins: impl IntoIterator<Item = DiagnosticOrigin>,
) -> Diagnostic {
    execution_error(
        "POP4008",
        "compileTime.attributeValidatorRejected",
        span,
        vec![DiagnosticArgument::Identifier(attribute.into())],
        origins,
    )
}

#[must_use]
pub fn invalid_attribute_validator_signature(
    span: SourceSpan,
    function: impl Into<String>,
) -> Diagnostic {
    execution_error(
        "POP4009",
        "compileTime.invalidAttributeValidatorSignature",
        span,
        vec![DiagnosticArgument::Identifier(function.into())],
        [],
    )
}
