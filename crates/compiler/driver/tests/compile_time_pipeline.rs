use pop_compile_time::{CompileTimeDependency, EvaluationFailureKind};
use pop_driver::{
    FrontEndBubbleInput, FrontEndCompileTimeEvaluation, FrontEndModule, FrontEndResult,
    analyze_bubble,
};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_hir::HirDeclarationKind;
use pop_mir::lower_hir_bubble;
use pop_source::SourceFile;
use pop_types::{
    AttributeConstant, AttributeQuerySubject, AttributeQueryValue, FieldDefault, IntegerKind,
};

const EXPLICIT: &str = include_str!("fixtures/compile_time/explicit.pop");
const UNMARKED: &str = include_str!("fixtures/compile_time/unmarked.pop");
const SPOOFED: &str = include_str!("fixtures/compile_time/spoofed.pop");
const TRANSITIVE_UNMARKED: &str = include_str!("fixtures/compile_time/transitive_unmarked.pop");
const SHORT_CIRCUIT: &str = include_str!("fixtures/compile_time/short_circuit.pop");
const CYCLE: &str = include_str!("fixtures/compile_time/cycle.pop");
const DECLARATION_ATTRIBUTES: &str =
    include_str!("fixtures/compile_time/declaration_attributes.pop");
const WRONG_TARGET: &str = include_str!("fixtures/compile_time/wrong_target.pop");

fn analyze(text: &str) -> FrontEndResult {
    let source = SourceFile::new(FileId::from_raw(0), "src/main.pop", text).expect("fixture");
    analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ))
}

#[test]
fn trusted_compile_time_functions_feed_udas_and_defaults_but_not_runtime_mir() {
    let result = analyze(EXPLICIT);
    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let hir = result.hir().expect("verified HIR");

    assert_eq!(
        hir.functions()
            .iter()
            .map(pop_hir::HirFunction::name)
            .collect::<Vec<_>>(),
        ["explicitAnswer", "defaultAnswer"]
    );
    let answer = hir
        .functions()
        .iter()
        .find(|function| function.name() == "explicitAnswer")
        .expect("runtime function");
    assert!(matches!(
        answer.attributes()[0].arguments()[0].value(),
        AttributeConstant::Integer(value)
            if value.kind() == IntegerKind::UInt8 && value.to_string() == "42"
    ));

    let settings = hir
        .declarations()
        .iter()
        .find(|declaration| declaration.name() == "Settings")
        .expect("record declaration");
    let HirDeclarationKind::Record(settings) = settings.kind() else {
        panic!("Settings record");
    };
    assert!(matches!(
        settings.fields()[0].default(),
        Some(FieldDefault::Integer(value))
            if value.kind() == IntegerKind::UInt8 && value.to_string() == "2"
    ));

    let mir = lower_hir_bubble(hir, result.types()).expect("runtime MIR");
    assert_eq!(mir.functions().len(), 2);
    assert_eq!(mir.declarations().len(), 1);
    assert!(
        result
            .compile_time_evaluations()
            .iter()
            .filter_map(FrontEndCompileTimeEvaluation::result)
            .all(|evaluation| evaluation
                .dependencies()
                .iter()
                .any(|dependency| matches!(dependency, CompileTimeDependency::Compiler { .. })))
    );
    assert!(
        result
            .compile_time_evaluations()
            .iter()
            .filter_map(FrontEndCompileTimeEvaluation::result)
            .any(
                |evaluation| evaluation.dependencies().iter().any(|dependency| matches!(
                    dependency,
                    CompileTimeDependency::CanonicalArguments { .. }
                ))
            )
    );
}

#[test]
fn unmarked_and_spoofed_compile_time_functions_are_rejected_by_identity() {
    for (name, source) in [
        ("unmarked", UNMARKED),
        ("spoofed", SPOOFED),
        ("transitive", TRANSITIVE_UNMARKED),
    ] {
        let result = analyze(source);
        assert!(result.hir().is_none(), "{name} source published HIR");
        assert!(
            result
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code().as_str() == "POP4004"),
            "{name}: {}",
            result.diagnostic_snapshot()
        );
    }
}

#[test]
fn compile_time_boolean_operators_preserve_source_short_circuiting() {
    let result = analyze(SHORT_CIRCUIT);
    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let hir = result.hir().expect("verified HIR");
    assert_eq!(hir.functions().len(), 1);
    assert!(matches!(
        hir.functions()[0].attributes()[0].arguments()[0].value(),
        AttributeConstant::Boolean(false)
    ));
}

#[test]
fn source_compile_time_cycles_report_the_cycle_with_call_and_request_provenance() {
    let result = analyze(CYCLE);
    assert!(result.hir().is_none());
    let diagnostic = result
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code().as_str() == "POP4006")
        .unwrap_or_else(|| panic!("{}", result.diagnostic_snapshot()));

    assert!(diagnostic.origin_chain().len() >= 2);
    assert!(
        diagnostic
            .origin_chain()
            .iter()
            .all(|origin| origin.kind() == pop_foundation::DiagnosticOriginKind::CompileTime)
    );
    let failure = result
        .compile_time_evaluations()
        .iter()
        .find_map(FrontEndCompileTimeEvaluation::failure)
        .expect("published failed evaluation");
    assert_eq!(failure.kind(), EvaluationFailureKind::CallCycle);
    assert_ne!(failure.origin(), failure.location());
    assert!(failure.call_chain().len() >= 2);
    assert!(
        failure.dependencies().iter().any(|dependency| matches!(
            dependency,
            CompileTimeDependency::CanonicalArguments { .. }
        ))
    );
}

#[test]
fn non_function_attributes_are_validated_and_queryable_in_source_order() {
    let result = analyze(DECLARATION_ATTRIBUTES);
    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let hir = result.hir().expect("verified HIR");
    let attribute = hir
        .declarations()
        .iter()
        .find(|declaration| declaration.name() == "Label")
        .expect("Label declaration");
    let HirDeclarationKind::Attribute(attribute_kind) = attribute.kind() else {
        panic!("Label attribute declaration");
    };
    let user = hir
        .declarations()
        .iter()
        .find(|declaration| declaration.name() == "User")
        .expect("User declaration");
    let queried = result
        .attribute_queries()
        .attribute(
            ModuleId::from_raw(0),
            AttributeQuerySubject::Symbol(user.symbol()),
            attribute_kind.attribute(),
        )
        .expect("visible resolved query");
    let AttributeQueryValue::ImmutableSequence(labels) = queried else {
        panic!("repeatable query result");
    };
    assert_eq!(labels.len(), 2);
    assert_eq!(
        labels
            .iter()
            .map(|label| label.arguments()[0].value())
            .collect::<Vec<_>>(),
        [
            &AttributeConstant::String("first".to_owned()),
            &AttributeConstant::String("second".to_owned()),
        ]
    );
}

#[test]
fn explicit_usage_rejects_the_wrong_source_target() {
    let result = analyze(WRONG_TARGET);
    assert!(result.hir().is_none());
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code().as_str() == "POP4001"),
        "{}",
        result.diagnostic_snapshot()
    );
}

#[test]
fn unattached_attribute_uses_are_never_silently_discarded() {
    let result = analyze("namespace Example\npublic attribute Marker()\n@Marker\n");

    assert!(result.hir().is_none());
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code().as_str() == "POP4001"),
        "{}",
        result.diagnostic_snapshot()
    );
}

#[test]
fn non_attribute_types_used_as_attributes_are_never_silently_discarded() {
    let result = analyze(
        "namespace Example\n\
         public record NotAttribute\n\
         end\n\
         @NotAttribute\n\
         public function invalid(): Int\n\
             return 0\n\
         end\n",
    );

    assert!(result.hir().is_none());
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code().as_str() == "POP4001"),
        "{}",
        result.diagnostic_snapshot()
    );
}
