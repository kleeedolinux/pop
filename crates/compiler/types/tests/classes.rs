use pop_foundation::{BubbleId, FileId, ModuleId};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{
    NodeKind, parse_class_declaration, parse_class_method_body, parse_file, parse_function_body,
    parse_function_signature,
};
use pop_types::{
    BodyChecker, ClassMethodDispatch, SemanticType, SignatureResolver, TypedExpressionKind,
    TypedStatementKind, embedded_bootstrap_schema,
};

fn define_class(text: &str) -> (pop_types::ClassDefinitionResult, pop_types::TypeArena) {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(FileId::from_raw(0), "src/counter.pop", text).expect("source");
    let syntax = parse_file(&source);
    let class_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::ClassDeclaration)
        .expect("class");
    let class_syntax = parse_class_declaration(&source, &syntax, class_node).expect("class syntax");
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.Counter", SymbolSpace::Type)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let result = resolver.define_class(module, symbol, &class_syntax);
    (result, resolver.into_arena())
}

#[test]
fn class_definitions_use_nominal_class_field_and_method_ids() {
    let (result, arena) = define_class(
        "namespace Example\n\
         public class Counter\n\
             private value: Int = 0\n\
             public function Counter.new(value: Int): Counter\n\
                 return Counter { value = value }\n\
             end\n\
             public function Counter:get(): Int\n\
                 return self.value\n\
             end\n\
         end\n",
    );

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let definition = result.definition().expect("class definition");
    assert!(!definition.is_open());
    assert_eq!(definition.class().raw(), 0);
    assert!(matches!(
        arena.get(definition.type_id()),
        Some(SemanticType::Class { class, arguments })
            if *class == definition.class() && arguments.is_empty()
    ));
    assert_eq!(definition.fields()[0].field().raw(), 0);
    assert_eq!(definition.fields()[0].name(), "value");
    assert!(definition.fields()[0].has_default());
    assert_eq!(definition.methods()[0].method().raw(), 0);
    assert_eq!(
        definition.methods()[0].dispatch(),
        ClassMethodDispatch::Static
    );
    assert_eq!(definition.methods()[1].method().raw(), 1);
    assert_eq!(
        definition.methods()[1].dispatch(),
        ClassMethodDispatch::Receiver
    );
}

#[test]
fn a_method_owner_must_match_the_enclosing_nominal_class() {
    let (result, _) = define_class(
        "namespace Example\n\
         public class Counter\n\
             public function Other:get(): Int\n\
                 return 0\n\
             end\n\
         end\n",
    );

    assert!(result.definition().is_none());
    assert!(result.diagnostic_snapshot().starts_with("POP2014"));
}

#[test]
fn class_field_defaults_share_typed_constant_evaluation() {
    let (result, _) = define_class(
        "namespace Example\n\
         public class Counter\n\
             public value: Int = 1 + 2\n\
         end\n",
    );
    assert!(result.diagnostics().is_empty());
    assert!(matches!(
        result.definition().expect("class").fields()[0].default(),
        Some(pop_types::FieldDefault::Integer(value)) if value.to_string() == "3"
    ));

    for (field, expected_code) in [
        ("public enabled: Boolean = 1", "POP2003"),
        ("public value: Int = runtimeValue()", "POP4001"),
    ] {
        let (result, _) = define_class(&format!(
            "namespace Example\n\
             public class Counter\n\
                 {field}\n\
             end\n"
        ));

        assert!(result.definition().is_none());
        assert!(
            result.diagnostic_snapshot().contains(expected_code),
            "{}",
            result.diagnostic_snapshot()
        );
    }
}

#[test]
fn class_construction_and_field_access_resolve_native_ids() {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/counter.pop",
        "namespace Example\n\
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
             local counter: Counter = Counter.new(value)\n\
             return counter:get()\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let class_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::ClassDeclaration)
        .expect("class");
    let function_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function");
    let class_syntax = parse_class_declaration(&source, &syntax, class_node).expect("class syntax");
    let function_syntax =
        parse_function_signature(&source, &syntax, function_node).expect("signature");
    let body =
        parse_function_body(&source, &syntax, function_node, &function_syntax).expect("body");
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let class_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.Counter", SymbolSpace::Type)[0]
        .symbol();
    let function_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.read", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let definition = resolver
        .define_class(module, class_symbol, &class_syntax)
        .definition()
        .expect("class definition")
        .clone();
    let signature = resolver
        .resolve(module, function_symbol, &function_syntax)
        .signature()
        .expect("signature")
        .clone();
    let signatures = std::collections::BTreeMap::from([(function_symbol, signature.clone())]);
    let result = BodyChecker::new(module, &mut resolver, &signatures).check(&signature, &body);

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let body = result.body().expect("typed body");
    let TypedStatementKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("local");
    };
    assert!(matches!(
        initializer.kind(),
        TypedExpressionKind::DirectMethodCall { method, receiver, .. }
            if *method == definition.methods()[0].method() && receiver.is_none()
    ));
    let TypedStatementKind::Return { values } = body.statements()[1].kind() else {
        panic!("return");
    };
    assert!(matches!(
        values[0].kind(),
        TypedExpressionKind::DirectMethodCall { method, receiver, .. }
            if *method == definition.methods()[1].method() && receiver.is_some()
    ));
}

#[test]
fn method_bodies_type_check_with_implicit_self_only_for_receiver_methods() {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/counter.pop",
        "namespace Example\n\
         public class Counter\n\
             public value: Int\n\
             public function Counter.new(value: Int): Counter\n\
                 return Counter { value = value }\n\
             end\n\
             public function Counter:get(): Int\n\
                 return self.value\n\
             end\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let class_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::ClassDeclaration)
        .expect("class");
    let class_syntax = parse_class_declaration(&source, &syntax, class_node).expect("class syntax");
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let class_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.Counter", SymbolSpace::Type)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let definition = resolver
        .define_class(module, class_symbol, &class_syntax)
        .definition()
        .expect("class definition")
        .clone();
    let no_functions = std::collections::BTreeMap::new();

    for (index, expected_parameter_count) in [(0, 1), (1, 1)] {
        let method = &definition.methods()[index];
        let signature = resolver.method_signature(&definition, method);
        assert_eq!(signature.parameters().len(), expected_parameter_count);
        assert_eq!(
            signature.parameters()[0].name(),
            if index == 0 { "value" } else { "self" }
        );
        let body =
            parse_class_method_body(&source, &syntax, class_node, &class_syntax.methods()[index])
                .expect("method body");
        let checked =
            BodyChecker::new(module, &mut resolver, &no_functions).check(&signature, &body);
        assert!(
            checked.diagnostics().is_empty(),
            "{}",
            checked.diagnostic_snapshot()
        );
        let TypedStatementKind::Return { values } =
            checked.body().expect("typed method").statements()[0].kind()
        else {
            panic!("method return");
        };
        if index == 0 {
            assert!(matches!(
                values[0].kind(),
                TypedExpressionKind::ClassConstruct { .. }
            ));
        } else {
            assert!(matches!(
                values[0].kind(),
                TypedExpressionKind::Field { base, .. }
                    if matches!(base.kind(), TypedExpressionKind::Parameter(parameter) if parameter.raw() == 0)
            ));
        }
    }
}

#[test]
fn generic_class_instances_specialize_fields_static_construction_and_receiver_methods() {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/box.pop",
        "namespace Example\n\
         private class Box<T>\n\
             private value: T\n\
             public function Box.new(value: T): Box<T>\n\
                 return Box { value = value }\n\
             end\n\
             public function Box:get(): T\n\
                 return self.value\n\
             end\n\
         end\n\
         public function read(value: Int): Int\n\
             local box: Box<Int> = Box.new(value)\n\
             return box:get()\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let class_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::ClassDeclaration)
        .expect("class");
    let function_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function");
    let class_syntax = parse_class_declaration(&source, &syntax, class_node).expect("class syntax");
    let function_syntax =
        parse_function_signature(&source, &syntax, function_node).expect("signature");
    let body =
        parse_function_body(&source, &syntax, function_node, &function_syntax).expect("body");
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let class_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.Box", SymbolSpace::Type)[0]
        .symbol();
    let function_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.read", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let template = resolver
        .define_class(module, class_symbol, &class_syntax)
        .definition()
        .expect("generic class template")
        .clone();
    let signature = resolver
        .resolve(module, function_symbol, &function_syntax)
        .signature()
        .expect("signature")
        .clone();
    let signatures = std::collections::BTreeMap::from([(function_symbol, signature.clone())]);
    let result = BodyChecker::new(module, &mut resolver, &signatures).check(&signature, &body);

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let typed = result.body().expect("typed body");
    let TypedStatementKind::Local { initializer, .. } = typed.statements()[0].kind() else {
        panic!("local");
    };
    let TypedExpressionKind::DirectMethodCall { method, .. } = initializer.kind() else {
        panic!("static construction");
    };
    let TypedStatementKind::Return { values } = typed.statements()[1].kind() else {
        panic!("return");
    };
    let TypedExpressionKind::DirectMethodCall {
        method: receiver_method,
        receiver: Some(receiver),
        ..
    } = values[0].kind()
    else {
        panic!("receiver method");
    };
    let instance = resolver
        .class_definition_for_type(receiver.type_id())
        .expect("concrete class instance");
    assert_ne!(instance.class(), template.class());
    assert_ne!(*method, template.methods()[0].method());
    assert_ne!(*receiver_method, template.methods()[1].method());
    assert_eq!(*method, instance.methods()[0].method());
    assert_eq!(*receiver_method, instance.methods()[1].method());
    assert_eq!(instance.fields()[0].field_type(), values[0].type_id());
}

#[test]
fn generic_class_arguments_require_exact_arity_and_satisfy_bounds() {
    for (annotation, expected_code) in [
        ("Box<Int, String>", "POP2001"),
        ("IterableBox<Int, Int>", "POP2028"),
    ] {
        let module = ModuleId::from_raw(0);
        let source = SourceFile::new(
            FileId::from_raw(0),
            "src/box.pop",
            format!(
                "namespace Example\n\
                 private class Box<T>\n\
                     private value: T\n\
                 end\n\
                 private class IterableBox<T, TSource: Iterable<T>>\n\
                     private value: TSource\n\
                 end\n\
                 public function reject(value: {annotation}): Int\n\
                     return 0\n\
                 end\n"
            ),
        )
        .expect("source");
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
        for class_node in syntax
            .root()
            .children()
            .iter()
            .filter(|node| node.kind() == NodeKind::ClassDeclaration)
        {
            let class_syntax =
                parse_class_declaration(&source, &syntax, class_node).expect("class syntax");
            let symbol = database.index().declaration_by_qualified_name(
                &format!("Example.{}", class_syntax.name()),
                SymbolSpace::Type,
            )[0]
            .symbol();
            let result = resolver.define_class(module, symbol, &class_syntax);
            assert!(
                result.diagnostics().is_empty(),
                "{}",
                result.diagnostic_snapshot()
            );
        }
        let function_node = syntax
            .root()
            .children()
            .iter()
            .find(|node| node.kind() == NodeKind::FunctionDeclaration)
            .expect("function");
        let function_syntax =
            parse_function_signature(&source, &syntax, function_node).expect("signature");
        let function_symbol = database
            .index()
            .declaration_by_qualified_name("Example.reject", SymbolSpace::Value)[0]
            .symbol();
        let result = resolver.resolve(module, function_symbol, &function_syntax);
        assert!(result.signature().is_none());
        assert!(
            result.diagnostic_snapshot().contains(expected_code),
            "{}",
            result.diagnostic_snapshot()
        );
    }
}
