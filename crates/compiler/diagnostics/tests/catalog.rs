use pop_diagnostics::{catalog, compile_time, syntax};
use pop_foundation::{
    DiagnosticArgument, DiagnosticCategory, DiagnosticOrigin, DiagnosticOriginKind,
    DiagnosticSeverity, FileId, SourceSpan, TextRange, TextSize,
};

fn span(start: u32, end: u32) -> SourceSpan {
    SourceSpan::new(
        FileId::from_raw(0),
        TextRange::new(TextSize::from_u32(start), TextSize::from_u32(end)).expect("ordered"),
    )
}

#[test]
fn catalog_is_sorted_unique_and_partitioned_by_compiler_phase() {
    let entries = catalog().expect("valid embedded diagnostic catalog");
    let codes: Vec<_> = entries.iter().map(|entry| entry.code().as_str()).collect();

    assert_eq!(
        codes,
        [
            "POP0001", "POP0002", "POP0003", "POP0004", "POP0006", "POP0007", "POP1000", "POP1001",
            "POP1002", "POP1003", "POP1004", "POP2001", "POP2002", "POP2003", "POP2004", "POP2005",
            "POP2006", "POP2007", "POP2008", "POP2009", "POP2010", "POP2011", "POP2012", "POP2013",
            "POP2014", "POP2015", "POP2016", "POP2017", "POP2018", "POP2019", "POP2020", "POP2021",
            "POP2022", "POP2023", "POP2024", "POP2025", "POP2026", "POP2027", "POP2028", "POP2029",
            "POP2030", "POP2031", "POP4001", "POP4002", "POP4003", "POP4004", "POP4005", "POP4006",
            "POP4007", "POP6400", "POP6401", "POP6402", "POP6403", "POP6404", "POP6405", "POP6406",
            "POP6407", "POP6408"
        ]
    );
    assert!(codes.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(
        entries[..6]
            .iter()
            .all(|entry| entry.category() == DiagnosticCategory::Syntax)
    );
    assert!(
        entries[6..11]
            .iter()
            .all(|entry| entry.category() == DiagnosticCategory::Resolution)
    );
    assert!(
        entries[11..42]
            .iter()
            .all(|entry| entry.category() == DiagnosticCategory::Type)
    );
    assert!(
        entries[42..49]
            .iter()
            .all(|entry| entry.category() == DiagnosticCategory::CompileTime)
    );
    assert!(
        entries[..49]
            .iter()
            .all(|entry| entry.warning_wave().is_none())
    );
    assert!(entries[..49].iter().all(|entry| !entry.is_suppressible()));
    assert!(entries[49..].iter().all(|entry| {
        entry.category() == DiagnosticCategory::Style
            && entry.severity() == DiagnosticSeverity::Warning
            && entry.warning_wave() == Some(1)
            && entry.is_suppressible()
    }));
    assert_eq!(entries[3].quick_fix_providers(), "replaceExportWithPublic");
}

#[test]
fn compile_time_execution_failures_preserve_typed_facts_and_provenance() {
    let call = DiagnosticOrigin::new(span(20, 24), DiagnosticOriginKind::CompileTime);
    let request = DiagnosticOrigin::new(span(40, 48), DiagnosticOriginKind::Source);
    let origins = [call, request];
    let diagnostics = [
        (
            compile_time::function_not_eligible(span(1, 5), "calculate", origins),
            "POP4004",
            vec![DiagnosticArgument::Identifier("calculate".to_owned())],
        ),
        (
            compile_time::forbidden_effect(span(6, 10), "load", "Filesystem", origins),
            "POP4005",
            vec![
                DiagnosticArgument::Identifier("load".to_owned()),
                DiagnosticArgument::Identifier("Filesystem".to_owned()),
            ],
        ),
        (
            compile_time::cycle(span(11, 15), "first -> second -> first", origins),
            "POP4006",
            vec![DiagnosticArgument::Identifier(
                "first -> second -> first".to_owned(),
            )],
        ),
        (
            compile_time::resource_limit(span(16, 19), "InstructionFuel", 10_000, origins),
            "POP4007",
            vec![
                DiagnosticArgument::Identifier("InstructionFuel".to_owned()),
                DiagnosticArgument::Unsigned(10_000),
            ],
        ),
    ];

    for (diagnostic, code, arguments) in diagnostics {
        assert_eq!(diagnostic.code().as_str(), code);
        assert_eq!(diagnostic.category(), DiagnosticCategory::CompileTime);
        assert_eq!(diagnostic.severity(), DiagnosticSeverity::Error);
        assert_eq!(diagnostic.arguments(), arguments);
        assert_eq!(diagnostic.origin_chain(), origins);
        assert!(diagnostic.fixes().is_empty());
        assert!(diagnostic.warning_wave().is_none());
        assert!(diagnostic.suppression_key().is_none());
    }
}

#[test]
fn required_constant_failures_use_structured_compile_time_diagnostics() {
    for diagnostic in [
        compile_time::ineligible_constant_expression(span(1, 5), "record field default"),
        compile_time::constant_integer_overflow(span(6, 10), "record field default"),
        compile_time::constant_division_by_zero(span(11, 15), "record field default"),
    ] {
        assert_eq!(diagnostic.category(), DiagnosticCategory::CompileTime);
        assert_eq!(diagnostic.severity(), DiagnosticSeverity::Error);
        assert_eq!(
            diagnostic.arguments(),
            &[DiagnosticArgument::Identifier(
                "record field default".to_owned()
            )]
        );
    }
}

#[test]
fn unsupported_export_uses_typed_arguments_and_a_safe_migration_fix() {
    let diagnostic = syntax::unsupported_export(span(15, 21));

    assert_eq!(diagnostic.code().as_str(), "POP0004");
    assert_eq!(diagnostic.severity(), DiagnosticSeverity::Error);
    assert_eq!(
        diagnostic.arguments(),
        &[DiagnosticArgument::Identifier("export".to_owned())]
    );
    let fix = diagnostic.fixes().first().expect("public migration fix");
    assert!(fix.is_safe());
    assert_eq!(fix.edit().edits()[0].replacement(), "public");
}
