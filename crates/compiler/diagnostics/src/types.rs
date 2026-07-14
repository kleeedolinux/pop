use pop_foundation::{
    Diagnostic, DiagnosticArgument, DiagnosticCategory, DiagnosticCode, DiagnosticLabel,
    DiagnosticSeverity, FixApplicability, MessageKey, QuickFix, SourceSpan, TextEdit,
    WorkspaceEdit,
};

#[must_use]
pub fn wrong_type_arity(
    span: SourceSpan,
    name: impl Into<String>,
    expected: u16,
    found: usize,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2001"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.wrongArity"),
        vec![
            DiagnosticArgument::Identifier(name.into()),
            DiagnosticArgument::Unsigned(u64::from(expected)),
            DiagnosticArgument::Unsigned(u64::try_from(found).unwrap_or(u64::MAX)),
        ],
        span,
    )
}

#[must_use]
pub fn duplicate_type_parameter(
    span: SourceSpan,
    name: impl Into<String>,
    original: SourceSpan,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2002"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.duplicateTypeParameter"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
    .with_label(DiagnosticLabel::new(
        original,
        MessageKey::new("types.originalTypeParameter"),
        Vec::new(),
    ))
}

#[must_use]
pub fn type_mismatch(
    span: SourceSpan,
    expected: impl Into<String>,
    found: impl Into<String>,
    expected_origin: SourceSpan,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2003"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.mismatch"),
        vec![
            DiagnosticArgument::Identifier(expected.into()),
            DiagnosticArgument::Identifier(found.into()),
        ],
        span,
    )
    .with_label(DiagnosticLabel::new(
        expected_origin,
        MessageKey::new("types.expectedOrigin"),
        Vec::new(),
    ))
}

#[must_use]
pub fn wrong_value_arity(
    span: SourceSpan,
    context: impl Into<String>,
    expected: usize,
    found: usize,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2004"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.wrongValueArity"),
        vec![
            DiagnosticArgument::Identifier(context.into()),
            DiagnosticArgument::Unsigned(u64::try_from(expected).unwrap_or(u64::MAX)),
            DiagnosticArgument::Unsigned(u64::try_from(found).unwrap_or(u64::MAX)),
        ],
        span,
    )
}

#[must_use]
pub fn no_matching_overload(
    span: SourceSpan,
    name: impl Into<String>,
    candidates: impl IntoIterator<Item = SourceSpan>,
) -> Diagnostic {
    candidates.into_iter().fold(
        Diagnostic::new(
            DiagnosticCode::new("POP2030"),
            DiagnosticSeverity::Error,
            DiagnosticCategory::Type,
            MessageKey::new("types.noMatchingOverload"),
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
pub fn invalid_overload_set(
    span: SourceSpan,
    name: impl Into<String>,
    reason: impl Into<String>,
    original: SourceSpan,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2031"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.invalidOverloadSet"),
        vec![
            DiagnosticArgument::Identifier(name.into()),
            DiagnosticArgument::Identifier(reason.into()),
        ],
        span,
    )
    .with_label(DiagnosticLabel::new(
        original,
        MessageKey::new("resolution.candidate"),
        Vec::new(),
    ))
}

#[must_use]
pub fn invalid_operator(
    span: SourceSpan,
    operator: impl Into<String>,
    operands: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2005"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.invalidOperator"),
        vec![
            DiagnosticArgument::Identifier(operator.into()),
            DiagnosticArgument::Identifier(operands.into()),
        ],
        span,
    )
}

#[must_use]
pub fn structural_mutation_during_iteration(
    span: SourceSpan,
    operation: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2029"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.structuralMutationDuringIteration"),
        vec![DiagnosticArgument::Identifier(operation.into())],
        span,
    )
}

#[must_use]
pub fn not_all_paths_return(span: SourceSpan) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2006"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.notAllPathsReturn"),
        Vec::new(),
        span,
    )
}

#[must_use]
pub fn aggregate_needs_context(span: SourceSpan) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2007"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.aggregateNeedsContext"),
        Vec::new(),
        span,
    )
}

#[must_use]
pub fn missing_record_field(span: SourceSpan, name: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2008"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.missingRecordField"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
}

#[must_use]
pub fn unknown_record_field(span: SourceSpan, name: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2009"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.unknownRecordField"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
}

#[must_use]
pub fn duplicate_record_field(
    span: SourceSpan,
    name: impl Into<String>,
    original: SourceSpan,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2010"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.duplicateRecordField"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
    .with_label(DiagnosticLabel::new(
        original,
        MessageKey::new("types.originalRecordField"),
        Vec::new(),
    ))
}

#[must_use]
pub fn duplicate_attribute_argument(
    span: SourceSpan,
    name: impl Into<String>,
    original: SourceSpan,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2011"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.duplicateAttributeArgument"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
    .with_label(DiagnosticLabel::new(
        original,
        MessageKey::new("types.originalAttributeArgument"),
        Vec::new(),
    ))
}

#[must_use]
pub fn unknown_attribute_argument(span: SourceSpan, name: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2012"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.unknownAttributeArgument"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
}

#[must_use]
pub fn numeric_literal_out_of_range(
    span: SourceSpan,
    literal: impl Into<String>,
    numeric_type: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2013"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.numericLiteralOutOfRange"),
        vec![
            DiagnosticArgument::Identifier(literal.into()),
            DiagnosticArgument::Identifier(numeric_type.into()),
        ],
        span,
    )
}

#[must_use]
pub fn wrong_class_method_owner(
    span: SourceSpan,
    expected: impl Into<String>,
    found: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2014"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.wrongClassMethodOwner"),
        vec![
            DiagnosticArgument::Identifier(expected.into()),
            DiagnosticArgument::Identifier(found.into()),
        ],
        span,
    )
}

#[must_use]
pub fn duplicate_interface_method(
    span: SourceSpan,
    name: impl Into<String>,
    original: SourceSpan,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2015"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.duplicateInterfaceMethod"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
    .with_label(DiagnosticLabel::new(
        original,
        MessageKey::new("types.originalInterfaceMethod"),
        Vec::new(),
    ))
}

#[must_use]
pub fn empty_interface(span: SourceSpan, name: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2016"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.emptyInterface"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
}

#[must_use]
pub fn invalid_interface_implementation(span: SourceSpan, target: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2017"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.invalidInterfaceImplementation"),
        vec![DiagnosticArgument::Identifier(target.into())],
        span,
    )
}

#[must_use]
pub fn missing_interface_method(
    span: SourceSpan,
    class: impl Into<String>,
    interface: impl Into<String>,
    method: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2018"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.missingInterfaceMethod"),
        vec![
            DiagnosticArgument::Identifier(class.into()),
            DiagnosticArgument::Identifier(interface.into()),
            DiagnosticArgument::Identifier(method.into()),
        ],
        span,
    )
}

#[must_use]
pub fn incompatible_interface_method(
    span: SourceSpan,
    class: impl Into<String>,
    interface: impl Into<String>,
    method: impl Into<String>,
    reason: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2019"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.incompatibleInterfaceMethod"),
        vec![
            DiagnosticArgument::Identifier(class.into()),
            DiagnosticArgument::Identifier(interface.into()),
            DiagnosticArgument::Identifier(method.into()),
            DiagnosticArgument::Identifier(reason.into()),
        ],
        span,
    )
}

#[must_use]
pub fn missing_match_cases(
    span: SourceSpan,
    cases: &[&str],
    insertion: SourceSpan,
    replacement: impl Into<String>,
) -> Diagnostic {
    let edit = TextEdit::new(insertion.file(), insertion.range(), replacement);
    Diagnostic::new(
        DiagnosticCode::new("POP2020"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.missingMatchCases"),
        vec![DiagnosticArgument::Identifier(cases.join(", "))],
        span,
    )
    .with_fix(QuickFix::new(
        "addMissingMatchCases",
        MessageKey::new("fix.addMissingMatchCases"),
        FixApplicability::Safe,
        WorkspaceEdit::new(0, vec![edit]),
    ))
}

#[must_use]
pub fn duplicate_match_case(
    span: SourceSpan,
    name: impl Into<String>,
    original: SourceSpan,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2021"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.duplicateMatchCase"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
    .with_label(DiagnosticLabel::new(
        original,
        MessageKey::new("types.originalMatchCase"),
        Vec::new(),
    ))
}

#[must_use]
pub fn foreign_match_case(span: SourceSpan, name: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2022"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.foreignMatchCase"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
}

#[must_use]
pub fn duplicate_binding(
    span: SourceSpan,
    name: impl Into<String>,
    original: SourceSpan,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2023"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.duplicateBinding"),
        vec![DiagnosticArgument::Identifier(name.into())],
        span,
    )
    .with_label(DiagnosticLabel::new(
        original,
        MessageKey::new("types.originalBinding"),
        Vec::new(),
    ))
}

#[must_use]
pub fn invalid_result_propagation(
    span: SourceSpan,
    operand: impl Into<String>,
    enclosing: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2024"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.invalidResultPropagation"),
        vec![
            DiagnosticArgument::Identifier(operand.into()),
            DiagnosticArgument::Identifier(enclosing.into()),
        ],
        span,
    )
}

#[must_use]
pub fn ambiguous_result_case(span: SourceSpan, case: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2025"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.ambiguousResultCase"),
        vec![DiagnosticArgument::Identifier(case.into())],
        span,
    )
}

#[must_use]
pub fn illegal_cleanup_control(span: SourceSpan, control: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2026"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.illegalCleanupControl"),
        vec![DiagnosticArgument::Identifier(control.into())],
        span,
    )
}

#[must_use]
pub fn invalid_generic_bound(
    span: SourceSpan,
    parameter: impl Into<String>,
    bound: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2027"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.invalidGenericBound"),
        vec![
            DiagnosticArgument::Identifier(parameter.into()),
            DiagnosticArgument::Identifier(bound.into()),
        ],
        span,
    )
}

#[must_use]
pub fn generic_inference_failure(
    span: SourceSpan,
    parameter: impl Into<String>,
    reason: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::new("POP2028"),
        DiagnosticSeverity::Error,
        DiagnosticCategory::Type,
        MessageKey::new("types.genericInferenceFailure"),
        vec![
            DiagnosticArgument::Identifier(parameter.into()),
            DiagnosticArgument::Identifier(reason.into()),
        ],
        span,
    )
}
