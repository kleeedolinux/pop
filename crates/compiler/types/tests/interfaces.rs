use pop_foundation::{BubbleId, FileId, ModuleId};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{
    NodeKind, SyntaxNode, SyntaxTree, parse_class_declaration, parse_file,
    parse_interface_declaration,
};
use pop_types::{
    ClassInterfaceImplementation, InterfaceDefinition, InterfaceMethodDefinition, SemanticType,
    SignatureResolver, embedded_bootstrap_schema,
};

type InterfaceIdentitySnapshot = (u32, u32, u32, Vec<u32>, Vec<u32>);

fn with_program<T>(
    text: &str,
    check: impl FnOnce(
        &SourceFile,
        &SyntaxTree,
        ModuleId,
        &ResolutionDatabase,
        &mut SignatureResolver<'_>,
    ) -> T,
) -> T {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(FileId::from_raw(0), "src/interfaces.pop", text).expect("source");
    let syntax = parse_file(&source);
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    check(&source, &syntax, module, &database, &mut resolver)
}

fn declaration_node(syntax: &SyntaxTree, kind: NodeKind, index: usize) -> &SyntaxNode {
    syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == kind)
        .nth(index)
        .expect("declaration node")
}

fn type_symbol(database: &ResolutionDatabase, qualified_name: &str) -> pop_foundation::SymbolId {
    database
        .index()
        .declaration_by_qualified_name(qualified_name, SymbolSpace::Type)[0]
        .symbol()
}

#[test]
fn interface_definitions_have_nominal_identity_and_typed_method_slots() {
    fn identities() -> Vec<InterfaceIdentitySnapshot> {
        with_program(
            "namespace Example\n\
             public interface Reader\n\
                 function read(count: Int): String\n\
                 function close()\n\
             end\n\
             public interface ReaderShape\n\
                 function read(count: Int): String\n\
                 function close()\n\
             end\n",
            |source, syntax, module, database, resolver| {
                (0..2)
                    .map(|index| {
                        let parsed = parse_interface_declaration(
                            source,
                            syntax,
                            declaration_node(syntax, NodeKind::InterfaceDeclaration, index),
                        )
                        .expect("interface syntax");
                        let symbol = type_symbol(
                            database,
                            if index == 0 {
                                "Example.Reader"
                            } else {
                                "Example.ReaderShape"
                            },
                        );
                        let result = resolver.define_interface(module, symbol, &parsed);
                        assert!(
                            result.diagnostics().is_empty(),
                            "{}",
                            result.diagnostic_snapshot()
                        );
                        let definition = result.definition().expect("interface");
                        assert!(matches!(
                            resolver.arena().get(definition.type_id()),
                            Some(SemanticType::Interface { interface, arguments })
                                if *interface == definition.interface() && arguments.is_empty()
                        ));
                        assert_eq!(
                            resolver
                                .interface_definition(symbol)
                                .map(InterfaceDefinition::interface),
                            Some(definition.interface())
                        );
                        assert_eq!(
                            resolver
                                .interface_definition_for_type(definition.type_id())
                                .map(InterfaceDefinition::interface),
                            Some(definition.interface())
                        );
                        assert_eq!(definition.methods()[0].name(), "read");
                        assert_eq!(definition.methods()[0].parameters().len(), 1);
                        assert_eq!(definition.methods()[0].results().len(), 1);
                        assert_eq!(definition.methods()[1].name(), "close");
                        assert!(definition.methods()[1].parameters().is_empty());
                        assert!(definition.methods()[1].results().is_empty());
                        (
                            definition.symbol().raw(),
                            definition.interface().raw(),
                            definition.type_id().raw(),
                            definition
                                .methods()
                                .iter()
                                .map(|method| method.method().raw())
                                .collect(),
                            definition
                                .methods()
                                .iter()
                                .map(InterfaceMethodDefinition::slot)
                                .collect(),
                        )
                    })
                    .collect()
            },
        )
    }

    let first = identities();
    let second = identities();
    assert_eq!(
        first, second,
        "interface/member identities must be deterministic"
    );
    assert_ne!(first[0].1, first[1].1, "shape must not determine identity");
    assert_eq!(first[0].4, vec![0, 1]);
    assert_eq!(first[1].4, vec![0, 1]);
}

#[test]
fn explicit_interface_and_slot_maps_are_canonicalized_by_nominal_identity() {
    with_program(
        "namespace Example\n\
         public interface Reader\n\
             function read(count: Int): String\n\
         end\n\
         public interface Closeable\n\
             function close()\n\
         end\n\
         public class Resource implements Closeable, Reader\n\
             public function Resource:close()\n\
             end\n\
             public function Resource:read(count: Int): String\n\
                 return \"\"\n\
             end\n\
         end\n",
        |source, syntax, module, database, resolver| {
            let mut interfaces = Vec::new();
            for (index, name) in ["Example.Reader", "Example.Closeable"]
                .into_iter()
                .enumerate()
            {
                let parsed = parse_interface_declaration(
                    source,
                    syntax,
                    declaration_node(syntax, NodeKind::InterfaceDeclaration, index),
                )
                .expect("interface syntax");
                let symbol = type_symbol(database, name);
                interfaces.push(
                    resolver
                        .define_interface(module, symbol, &parsed)
                        .definition()
                        .expect("interface")
                        .clone(),
                );
            }

            let class_syntax = parse_class_declaration(
                source,
                syntax,
                declaration_node(syntax, NodeKind::ClassDeclaration, 0),
            )
            .expect("class syntax");
            let class_symbol = type_symbol(database, "Example.Resource");
            let result = resolver.define_class(module, class_symbol, &class_syntax);
            assert!(
                result.diagnostics().is_empty(),
                "{}",
                result.diagnostic_snapshot()
            );
            let class = result.definition().expect("class");

            assert_eq!(
                class
                    .interfaces()
                    .iter()
                    .map(ClassInterfaceImplementation::interface)
                    .collect::<Vec<_>>(),
                interfaces
                    .iter()
                    .map(InterfaceDefinition::interface)
                    .collect::<Vec<_>>()
            );
            for implementation in class.interfaces() {
                assert_eq!(implementation.methods().len(), 1);
                assert_eq!(implementation.methods()[0].slot(), 0);
            }
        },
    );
}

#[test]
fn exact_explicit_class_implementation_records_nominal_slot_mappings_and_upcast() {
    with_program(
        "namespace Example\n\
         public interface Reader\n\
             function read(count: Int): String\n\
         end\n\
         public class FileReader implements Reader\n\
             public function FileReader:read(count: Int): String\n\
                 return \"\"\n\
             end\n\
         end\n",
        |source, syntax, module, database, resolver| {
            let interface_syntax = parse_interface_declaration(
                source,
                syntax,
                declaration_node(syntax, NodeKind::InterfaceDeclaration, 0),
            )
            .expect("interface syntax");
            let interface_symbol = type_symbol(database, "Example.Reader");
            let interface = resolver
                .define_interface(module, interface_symbol, &interface_syntax)
                .definition()
                .expect("interface")
                .clone();
            let class_syntax = parse_class_declaration(
                source,
                syntax,
                declaration_node(syntax, NodeKind::ClassDeclaration, 0),
            )
            .expect("class syntax");
            let class_symbol = type_symbol(database, "Example.FileReader");
            let result = resolver.define_class(module, class_symbol, &class_syntax);

            assert!(
                result.diagnostics().is_empty(),
                "{}",
                result.diagnostic_snapshot()
            );
            let class = result.definition().expect("class");
            assert_eq!(class.interfaces().len(), 1);
            let implementation = &class.interfaces()[0];
            assert_eq!(implementation.interface(), interface.interface());
            assert_eq!(implementation.interface_type(), interface.type_id());
            assert_eq!(implementation.methods().len(), 1);
            assert_eq!(
                implementation.methods()[0].interface_method(),
                interface.methods()[0].method()
            );
            assert_eq!(
                implementation.methods()[0].class_method(),
                class.methods()[0].method()
            );
            assert!(resolver.class_implements_interface(class.class(), interface.interface()));
            assert!(resolver.is_class_to_interface_upcast(class.type_id(), interface.type_id()));
        },
    );
}

#[test]
fn missing_mismatched_static_and_inaccessible_methods_are_rejected() {
    for (method, expected) in [
        ("", "POP2018"),
        (
            "public function FileReader:read(count: Boolean): String\nreturn \"\"\nend",
            "POP2019",
        ),
        (
            "public function FileReader.read(count: Int): String\nreturn \"\"\nend",
            "POP2019",
        ),
        (
            "private function FileReader:read(count: Int): String\nreturn \"\"\nend",
            "POP2019",
        ),
    ] {
        with_program(
            &format!(
                "namespace Example\n\
                 public interface Reader\n\
                     function read(count: Int): String\n\
                 end\n\
                 public class FileReader implements Reader\n\
                     {method}\n\
                 end\n"
            ),
            |source, syntax, module, database, resolver| {
                let interface_syntax = parse_interface_declaration(
                    source,
                    syntax,
                    declaration_node(syntax, NodeKind::InterfaceDeclaration, 0),
                )
                .expect("interface syntax");
                let interface_symbol = type_symbol(database, "Example.Reader");
                assert!(
                    resolver
                        .define_interface(module, interface_symbol, &interface_syntax)
                        .diagnostics()
                        .is_empty()
                );
                let class_syntax = parse_class_declaration(
                    source,
                    syntax,
                    declaration_node(syntax, NodeKind::ClassDeclaration, 0),
                )
                .expect("class syntax");
                let class_symbol = type_symbol(database, "Example.FileReader");
                let result = resolver.define_class(module, class_symbol, &class_syntax);

                assert!(result.definition().is_none());
                assert!(
                    result.diagnostic_snapshot().contains(expected),
                    "{}",
                    result.diagnostic_snapshot()
                );
            },
        );
    }
}

#[test]
fn matching_shape_without_implements_does_not_create_an_interface_relation() {
    with_program(
        "namespace Example\n\
         public interface Reader\n\
             function read(count: Int): String\n\
         end\n\
         public class ShapeOnlyReader\n\
             public function ShapeOnlyReader:read(count: Int): String\n\
                 return \"\"\n\
             end\n\
         end\n",
        |source, syntax, module, database, resolver| {
            let interface_syntax = parse_interface_declaration(
                source,
                syntax,
                declaration_node(syntax, NodeKind::InterfaceDeclaration, 0),
            )
            .expect("interface syntax");
            let interface_symbol = type_symbol(database, "Example.Reader");
            let interface = resolver
                .define_interface(module, interface_symbol, &interface_syntax)
                .definition()
                .expect("interface")
                .clone();
            let class_syntax = parse_class_declaration(
                source,
                syntax,
                declaration_node(syntax, NodeKind::ClassDeclaration, 0),
            )
            .expect("class syntax");
            let class_symbol = type_symbol(database, "Example.ShapeOnlyReader");
            let result = resolver.define_class(module, class_symbol, &class_syntax);
            let class = result.definition().expect("shape-only class remains valid");

            assert!(class.interfaces().is_empty());
            assert!(!resolver.class_implements_interface(class.class(), interface.interface()));
            assert!(!resolver.is_class_to_interface_upcast(class.type_id(), interface.type_id()));
        },
    );
}

#[test]
fn invalid_interface_declarations_and_implements_targets_are_structured_errors() {
    for (interface_body, expected) in [
        ("", "POP2016"),
        (
            "function read(count: Int): String\nfunction read(count: Int): String",
            "POP2015",
        ),
    ] {
        with_program(
            &format!(
                "namespace Example\n\
                 public interface Reader\n\
                     {interface_body}\n\
                 end\n"
            ),
            |source, syntax, module, database, resolver| {
                let parsed = parse_interface_declaration(
                    source,
                    syntax,
                    declaration_node(syntax, NodeKind::InterfaceDeclaration, 0),
                )
                .expect("interface syntax");
                let symbol = type_symbol(database, "Example.Reader");
                let result = resolver.define_interface(module, symbol, &parsed);
                assert!(result.definition().is_none());
                assert!(
                    result.diagnostic_snapshot().contains(expected),
                    "{}",
                    result.diagnostic_snapshot()
                );
            },
        );
    }

    with_program(
        "namespace Example\n\
         public record ReaderShape\n\
         end\n\
         public class FileReader implements ReaderShape\n\
         end\n",
        |source, syntax, module, database, resolver| {
            let record_node = declaration_node(syntax, NodeKind::RecordDeclaration, 0);
            let record_syntax = pop_syntax::parse_record_declaration(source, syntax, record_node)
                .expect("record syntax");
            let record_symbol = type_symbol(database, "Example.ReaderShape");
            assert!(
                resolver
                    .define_record(module, record_symbol, &record_syntax)
                    .diagnostics()
                    .is_empty()
            );
            let class_syntax = parse_class_declaration(
                source,
                syntax,
                declaration_node(syntax, NodeKind::ClassDeclaration, 0),
            )
            .expect("class syntax");
            let class_symbol = type_symbol(database, "Example.FileReader");
            let result = resolver.define_class(module, class_symbol, &class_syntax);

            assert!(result.definition().is_none());
            assert!(
                result.diagnostic_snapshot().contains("POP2017"),
                "{}",
                result.diagnostic_snapshot()
            );
        },
    );
}

#[test]
fn duplicate_explicit_interface_implementations_are_rejected() {
    with_program(
        "namespace Example\n\
         public interface Reader\n\
             function read(count: Int): String\n\
         end\n\
         public class FileReader implements Reader, Reader\n\
             public function FileReader:read(count: Int): String\n\
                 return \"\"\n\
             end\n\
         end\n",
        |source, syntax, module, database, resolver| {
            let interface_syntax = parse_interface_declaration(
                source,
                syntax,
                declaration_node(syntax, NodeKind::InterfaceDeclaration, 0),
            )
            .expect("interface syntax");
            let interface_symbol = type_symbol(database, "Example.Reader");
            assert!(
                resolver
                    .define_interface(module, interface_symbol, &interface_syntax)
                    .diagnostics()
                    .is_empty()
            );
            let class_syntax = parse_class_declaration(
                source,
                syntax,
                declaration_node(syntax, NodeKind::ClassDeclaration, 0),
            )
            .expect("class syntax");
            let class_symbol = type_symbol(database, "Example.FileReader");
            let result = resolver.define_class(module, class_symbol, &class_syntax);

            assert!(result.definition().is_none());
            assert!(
                result.diagnostic_snapshot().contains("POP2017"),
                "{}",
                result.diagnostic_snapshot()
            );
        },
    );
}
