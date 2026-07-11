use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_hir::{HirCallDispatch, HirDeclarationKind, HirExpressionKind, HirStatementKind};
use pop_mir::lower_hir_bubble;
use pop_source::SourceFile;

#[test]
fn multi_module_bubble_reaches_verified_typed_hir() {
    let models = SourceFile::new(
        FileId::from_raw(0),
        "src/models.pop",
        "namespace Game.Models\n\
         public record Player\n\
             name: String\n\
         end\n",
    )
    .expect("models");
    let service = SourceFile::new(
        FileId::from_raw(1),
        "src/service.pop",
        "namespace Game.Service\n\
         using Game.Models\n\
         public function identity(player: Player): Player\n\
             return player\n\
         end\n",
    )
    .expect("service");
    let input = FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![
            FrontEndModule::new(ModuleId::from_raw(1), service),
            FrontEndModule::new(ModuleId::from_raw(0), models),
        ],
    );
    let result = analyze_bubble(input);

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let hir = result.hir().expect("HIR");
    assert_eq!(hir.functions().len(), 1);
    assert_eq!(hir.declarations().len(), 1);
    assert!(matches!(
        hir.declarations()[0].kind(),
        HirDeclarationKind::Record(_)
    ));
    assert_eq!(hir.public_symbols().len(), 2);
    assert!(hir.dump(result.types()).contains("identity"));
}

#[test]
fn standard_print_is_identity_bound_and_survives_hir_and_mir() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\npublic function run(): Int\n    print(42)\n    return 0\nend\n",
    )
    .expect("source");
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    assert!(result.diagnostics().is_empty(), "{}", result.diagnostic_snapshot());
    let hir = result.hir().expect("HIR");
    assert!(hir.dump(result.types()).contains("call.standard sf0"));
    let mir = lower_hir_bubble(hir, result.types()).expect("verified MIR");
    let dump = mir.dump();
    assert!(dump.contains("callStandard sf0"));
    assert!(!dump.contains("pop_std_print_int"));
    assert_eq!(pop_mir::parse_mir_dump(&dump).expect("round trip"), mir);
}

#[test]
fn standard_print_rejects_wrong_calls_and_nearer_declarations_shadow_it() {
    for body in ["print()", "print(true)"] {
        let source = SourceFile::new(
            FileId::from_raw(0),
            "src/invalid.pop",
            format!("namespace Main\npublic function run(): Int\n    {body}\n    return 0\nend\n"),
        )
        .expect("source");
        let result = analyze_bubble(FrontEndBubbleInput::new(
            BubbleId::from_raw(0),
            NamespaceId::from_raw(0),
            Vec::new(),
            vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
        ));
        assert!(!result.diagnostics().is_empty());
        assert!(result.hir().is_none());
    }

    let source = SourceFile::new(
        FileId::from_raw(1),
        "src/shadow.pop",
        "namespace Main\npublic function print(value: Int): Int\n    return value\nend\npublic function run(): Int\n    return print(42)\nend\n",
    )
    .expect("source");
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    assert!(result.diagnostics().is_empty(), "{}", result.diagnostic_snapshot());
    let dump = result.hir().expect("HIR").dump(result.types());
    assert!(dump.contains("call.direct s0"));
    assert!(!dump.contains("call.standard"));
}

#[test]
fn hir_retains_existing_type_declarations_and_visibility_derived_public_surface() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/declarations.pop",
        "namespace Main\n\
         public attribute Marker(value: Int = 1)\n\
         internal record InternalData\n\
             value: Int = 1\n\
         end\n\
         private union Secret\n\
             Hidden\n\
         end\n\
         public class Counter\n\
             public value: Int\n\
         end\n\
         public function read(counter: Counter): Int\n\
             return counter.value\n\
         end\n",
    )
    .expect("source");
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let hir = result.hir().expect("HIR");
    assert_eq!(hir.declarations().len(), 4);
    assert_eq!(
        hir.declarations()
            .iter()
            .map(pop_hir::HirDeclaration::name)
            .collect::<Vec<_>>(),
        ["Marker", "InternalData", "Secret", "Counter"]
    );
    assert!(matches!(
        hir.declarations()[0].kind(),
        HirDeclarationKind::Attribute(_)
    ));
    assert!(matches!(
        hir.declarations()[1].kind(),
        HirDeclarationKind::Record(_)
    ));
    let HirDeclarationKind::Record(record) = hir.declarations()[1].kind() else {
        panic!("record declaration");
    };
    assert!(matches!(
        record.fields()[0].default(),
        Some(pop_types::FieldDefault::Integer(value)) if value.to_string() == "1"
    ));
    assert!(matches!(
        hir.declarations()[2].kind(),
        HirDeclarationKind::Union(_)
    ));
    assert!(matches!(
        hir.declarations()[3].kind(),
        HirDeclarationKind::Class(_)
    ));
    assert_eq!(hir.public_symbols().len(), 3);
    let dump = hir.dump(result.types());
    assert!(dump.contains("attribute Marker"));
    assert!(dump.contains("record InternalData"));
    assert!(dump.contains("union Secret"));
    assert!(dump.contains("class Counter"));
}

#[test]
fn same_shaped_record_declarations_share_structural_field_identity() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/records.pop",
        "namespace Main\n\
         public record First\n\
             value: Int = 1\n\
         end\n\
         public record Second\n\
             value: Int = 2\n\
         end\n",
    )
    .expect("source");
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));

    assert!(result.diagnostics().is_empty());
    let declarations = result.hir().expect("HIR").declarations();
    let HirDeclarationKind::Record(first) = declarations[0].kind() else {
        panic!("First record");
    };
    let HirDeclarationKind::Record(second) = declarations[1].kind() else {
        panic!("Second record");
    };
    assert_eq!(first.type_id(), second.type_id());
    assert_eq!(first.fields()[0].field(), second.fields()[0].field());
}

#[test]
fn semantic_errors_prevent_hir_publication_without_runtime_lookup_fallback() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\n\
         public function invalid(): Int\n\
             return missingValue\n\
         end\n",
    )
    .expect("source");
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));

    assert!(result.hir().is_none());
    assert!(result.diagnostic_snapshot().starts_with("POP1002"));
}

#[test]
fn zero_result_calls_are_rejected_only_when_a_value_is_required() {
    for body in ["local value = observe(1)\nreturn 0", "return observe(1)"] {
        let source = SourceFile::new(
            FileId::from_raw(0),
            "src/resultless.pop",
            format!(
                "namespace Main\n\
                 private function observe(value: Int)\n\
                     value\n\
                 end\n\
                 public function invalid(): Int\n\
                     {body}\n\
                 end\n"
            ),
        )
        .expect("source");
        let result = analyze_bubble(FrontEndBubbleInput::new(
            BubbleId::from_raw(0),
            NamespaceId::from_raw(0),
            Vec::new(),
            vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
        ));

        assert!(result.hir().is_none());
        assert!(result.diagnostic_snapshot().contains("POP2004"));
    }
}

#[test]
fn hir_retains_zero_result_calls_as_effect_statements() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/resultless.pop",
        "namespace Main\n\
         private function observe(value: Int)\n\
             value\n\
         end\n\
         public function run()\n\
             observe(1)\n\
         end\n",
    )
    .expect("source");
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));

    assert!(result.diagnostics().is_empty());
    let HirStatementKind::Call(call) = result.hir().expect("HIR").functions()[1].body()[0].kind()
    else {
        panic!("zero-result call statement");
    };
    assert!(matches!(
        call.dispatch(),
        HirCallDispatch::Direct { function } if function.raw() == 0
    ));
    assert_eq!(call.arguments().len(), 1);
    assert!(
        result
            .hir()
            .expect("HIR")
            .dump(result.types())
            .contains("do call.direct s0")
    );
}

#[test]
fn native_class_construction_reaches_hir_as_a_class_operation() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/counter.pop",
        "namespace Main\n\
         public class Counter\n\
             public value: Int\n\
             public function Counter.new(value: Int): Counter\n\
                 return Counter { value = value }\n\
             end\n\
             public function Counter:get(): Int\n\
                 return self.value\n\
             end\n\
         end\n\
         public function read(value: Int): Int\n\
             local counter = Counter.new(value)\n\
             return counter:get()\n\
         end\n",
    )
    .expect("source");
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let hir = result.hir().expect("HIR");
    assert_eq!(hir.methods().len(), 2);
    let HirStatementKind::Local { initializer, .. } = hir.functions()[0].body()[0].kind() else {
        panic!("local");
    };
    assert!(matches!(
        initializer.kind(),
        HirExpressionKind::Call {
            dispatch: pop_hir::HirCallDispatch::DirectMethod { method },
            ..
        } if *method == hir.methods()[0].method()
    ));
    let HirStatementKind::Return { values } = hir.functions()[0].body()[1].kind() else {
        panic!("return");
    };
    assert!(matches!(
        values[0].kind(),
        HirExpressionKind::Call {
            dispatch: pop_hir::HirCallDispatch::DirectMethod { method },
            ..
        } if *method == hir.methods()[1].method()
    ));
}

#[test]
fn private_class_members_stop_at_the_declaring_module() {
    for body in [
        "return Vault.secret()",
        "local vault = Vault.new(1)\n             return vault.value",
    ] {
        let model = SourceFile::new(
            FileId::from_raw(0),
            "src/vault.pop",
            "namespace Model\n\
             public class Vault\n\
                 private value: Int\n\
                 private function Vault.secret(): Int\n\
                     return 1\n\
                 end\n\
                 public function Vault.new(value: Int): Vault\n\
                     return Vault { value = value }\n\
                 end\n\
             end\n",
        )
        .expect("model");
        let service = SourceFile::new(
            FileId::from_raw(1),
            "src/service.pop",
            format!(
                "namespace Service\n\
                 using Model\n\
                 public function invalid(): Int\n\
                     {body}\n\
                 end\n"
            ),
        )
        .expect("service");
        let result = analyze_bubble(FrontEndBubbleInput::new(
            BubbleId::from_raw(0),
            NamespaceId::from_raw(0),
            Vec::new(),
            vec![
                FrontEndModule::new(ModuleId::from_raw(0), model),
                FrontEndModule::new(ModuleId::from_raw(1), service),
            ],
        ));

        assert!(result.hir().is_none());
        assert!(result.diagnostic_snapshot().contains("POP1004"));
    }
}

#[test]
fn assignment_is_limited_to_typed_native_class_fields() {
    for (source_text, expected_code) in [
        (
            "namespace Main\n\
             public record Score\n\
                 value: Int\n\
             end\n\
             public function invalid(): Int\n\
                 local score: Score = { value = 1 }\n\
                 score.value = 2\n\
                 return score.value\n\
             end\n",
            "POP2005",
        ),
        (
            "namespace Main\n\
             public function invalid(value: Int): Int\n\
                 value = 2\n\
                 return value\n\
             end\n",
            "POP2005",
        ),
        (
            "namespace Main\n\
             public class Counter\n\
                 public value: Int\n\
             end\n\
             public function invalid(counter: Counter): Int\n\
                 counter.value = \"wrong\"\n\
                 return counter.value\n\
             end\n",
            "POP2003",
        ),
    ] {
        let source =
            SourceFile::new(FileId::from_raw(0), "src/main.pop", source_text).expect("source");
        let result = analyze_bubble(FrontEndBubbleInput::new(
            BubbleId::from_raw(0),
            NamespaceId::from_raw(0),
            Vec::new(),
            vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
        ));

        assert!(result.hir().is_none());
        assert!(
            result.diagnostic_snapshot().contains(expected_code),
            "{}",
            result.diagnostic_snapshot()
        );
    }
}

#[test]
fn indirect_calls_keep_the_declared_function_arity() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\n\
         public function invalid(operation: function(value: Int): Int): Int\n\
             return operation()\n\
         end\n",
    )
    .expect("source");
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));

    assert!(result.hir().is_none());
    assert!(
        result.diagnostic_snapshot().contains("POP2004"),
        "{}",
        result.diagnostic_snapshot()
    );
}

#[test]
fn mutable_collections_and_functions_do_not_gain_structural_equality() {
    for parameter_type in ["{Int}", "{[String]: Int}", "function(value: Int): Int"] {
        let source = SourceFile::new(
            FileId::from_raw(0),
            "src/main.pop",
            format!(
                "namespace Main\n\
                 public function invalid(left: {parameter_type}, right: {parameter_type}): Boolean\n\
                     return left == right\n\
                 end\n"
            ),
        )
        .expect("source");
        let result = analyze_bubble(FrontEndBubbleInput::new(
            BubbleId::from_raw(0),
            NamespaceId::from_raw(0),
            Vec::new(),
            vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
        ));

        assert!(result.hir().is_none());
        assert!(
            result.diagnostic_snapshot().contains("POP2005"),
            "{}",
            result.diagnostic_snapshot()
        );
    }
}

#[test]
fn source_interfaces_are_nominal_and_dispatch_by_resolved_slot() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/reader.pop",
        "namespace Main\n\
         private interface Closeable\n\
             function close()\n\
         end\n\
         public interface Reader\n\
             function read(count: Int): String\n\
         end\n\
         public class FileReader implements Reader\n\
             public function FileReader:read(count: Int): String\n\
                 return \"\"\n\
             end\n\
         end\n\
         public function readOne(reader: FileReader): String\n\
             local contract: Reader = reader\n\
             return contract:read(1)\n\
         end\n",
    )
    .expect("source");
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let hir = result.hir().expect("verified HIR");
    let reader = hir
        .declarations()
        .iter()
        .find(|declaration| declaration.name() == "Reader")
        .and_then(pop_hir::HirDeclaration::as_interface)
        .expect("Reader interface");
    assert_eq!(reader.methods()[0].slot(), 0);
    assert_ne!(
        reader.methods()[0].method().raw(),
        reader.methods()[0].slot()
    );
    let HirStatementKind::Local { initializer, .. } = hir.functions()[0].body()[0].kind() else {
        panic!("interface upcast local");
    };
    assert!(matches!(
        initializer.kind(),
        HirExpressionKind::InterfaceUpcast { interface, .. }
            if *interface == reader.interface()
    ));
    let HirStatementKind::Return { values } = hir.functions()[0].body()[1].kind() else {
        panic!("interface call return");
    };
    assert!(matches!(
        values[0].kind(),
        HirExpressionKind::Call {
            dispatch: HirCallDispatch::InterfaceMethod {
                interface,
                method,
                slot: 0,
            },
            ..
        } if *interface == reader.interface() && *method == reader.methods()[0].method()
    ));
    let dump = hir.dump(result.types());
    assert!(dump.contains("interface Reader"), "{dump}");
    assert!(dump.contains("convert.interface"), "{dump}");
    assert!(dump.contains("call.interface"), "{dump}");
    assert!(!dump.to_ascii_lowercase().contains("lookup name"), "{dump}");
}

#[test]
fn source_interface_resolution_is_independent_of_module_order() {
    let implementation = SourceFile::new(
        FileId::from_raw(0),
        "src/fileReader.pop",
        "namespace Main\n\
         using Contracts\n\
         public class FileReader implements Reader\n\
             public function FileReader:read(count: Int): String\n\
                 return \"\"\n\
             end\n\
         end\n",
    )
    .expect("implementation");
    let contract = SourceFile::new(
        FileId::from_raw(1),
        "src/reader.pop",
        "namespace Contracts\n\
         public interface Reader\n\
             function read(count: Int): String\n\
         end\n",
    )
    .expect("contract");
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![
            FrontEndModule::new(ModuleId::from_raw(0), implementation),
            FrontEndModule::new(ModuleId::from_raw(1), contract),
        ],
    ));

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let hir = result.hir().expect("verified HIR");
    assert_eq!(
        hir.declarations()
            .iter()
            .filter(|declaration| matches!(declaration.kind(), HirDeclarationKind::Interface(_)))
            .count(),
        1
    );
    let class = hir
        .declarations()
        .iter()
        .find_map(pop_hir::HirDeclaration::as_class)
        .expect("class");
    assert_eq!(class.interfaces().len(), 1);
    assert_eq!(class.interfaces()[0].methods().len(), 1);
}

#[test]
fn explicit_interface_implementation_is_required_and_exact() {
    let cases = [
        (
            "public class FileReader implements Reader\n\
                 end",
            "POP2018",
        ),
        (
            "public class FileReader implements Reader\n\
                     public function FileReader:read(count: Int): Boolean\n\
                         return false\n\
                     end\n\
                 end",
            "POP2019",
        ),
        (
            "public class FileReader\n\
                     public function FileReader:read(count: Int): String\n\
                         return \"\"\n\
                     end\n\
                 end\n\
                 public function asReader(reader: FileReader): Reader\n\
                     return reader\n\
                 end",
            "POP2003",
        ),
    ];
    for (declarations, diagnostic) in cases {
        let source = SourceFile::new(
            FileId::from_raw(0),
            "src/invalidInterface.pop",
            format!(
                "namespace Main\n\
                 public interface Reader\n\
                     function read(count: Int): String\n\
                 end\n\
                 {declarations}\n"
            ),
        )
        .expect("source");
        let result = analyze_bubble(FrontEndBubbleInput::new(
            BubbleId::from_raw(0),
            NamespaceId::from_raw(0),
            Vec::new(),
            vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
        ));

        assert!(result.hir().is_none());
        assert!(
            result.diagnostic_snapshot().contains(diagnostic),
            "{}",
            result.diagnostic_snapshot()
        );
    }
}

#[test]
fn source_closures_and_exhaustive_matches_reach_verified_hir() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/closureMatch.pop",
        "namespace Main\n\
         public union Choice\n\
             Some(value: Int)\n\
             None\n\
         end\n\
         public function run(choice: Choice, offset: Int): Int\n\
             local function add(value: Int): Int\n\
                 return value + offset\n\
             end\n\
             match choice\n\
             when Choice.Some(value) then\n\
                 return add(value)\n\
             when Choice.None then\n\
                 return 0\n\
             end\n\
         end\n",
    )
    .expect("source");
    let result = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let dump = result.hir().expect("verified HIR").dump(result.types());
    assert!(dump.contains("closure"), "{dump}");
    assert!(dump.contains("match"), "{dump}");
}
