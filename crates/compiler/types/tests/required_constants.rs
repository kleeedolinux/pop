use pop_diagnostics::compile_time as compile_time_diagnostics;
use pop_foundation::{BubbleId, FileId, ModuleId, SymbolId};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{
    ExpressionSyntaxKind, NodeKind, parse_attribute_declaration, parse_attribute_use,
    parse_class_declaration, parse_file, parse_record_declaration,
};
use pop_types::{
    AttributeConstant, AttributeDefinitionResult, ClassDefinitionResult, FieldDefault, IntegerKind,
    IntegerValue, RecordDefinitionResult, RequiredConstantError, RequiredConstantTarget,
    SignatureResolver, embedded_bootstrap_schema,
};

#[test]
fn schemas_retain_pending_defaults_until_canonical_values_are_installed_by_identity() {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/requiredConstants.pop",
        "namespace Example\n\
         public attribute Build(explicit: UInt8, fallback: UInt8 = calculate())\n\
         public record Settings\n\
             count: UInt8 = calculate()\n\
         end\n\
         public class Counter\n\
             public count: UInt8 = calculate()\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let attribute_syntax = parse_attribute_declaration(
        &source,
        &syntax,
        node(&syntax, NodeKind::AttributeDeclaration),
    )
    .expect("attribute syntax");
    let record_syntax =
        parse_record_declaration(&source, &syntax, node(&syntax, NodeKind::RecordDeclaration))
            .expect("record syntax");
    let class_syntax =
        parse_class_declaration(&source, &syntax, node(&syntax, NodeKind::ClassDeclaration))
            .expect("class syntax");
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let attribute_symbol = symbol(&indexed, "Example.Build");
    let record_symbol = symbol(&indexed, "Example.Settings");
    let class_symbol = symbol(&indexed, "Example.Counter");
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));

    let attribute = resolver.define_attribute_schema(module, attribute_symbol, &attribute_syntax);
    let record = resolver.define_record_schema(module, record_symbol, &record_syntax);
    let class = resolver.define_class_schema(module, class_symbol, &class_syntax);
    assert!(attribute.diagnostics().is_empty());
    assert!(record.diagnostics().is_empty());
    assert!(class.diagnostics().is_empty());

    assert_pending_defaults(&attribute, &record, &class);
    install_and_assert_defaults(
        &mut resolver,
        attribute_symbol,
        record_symbol,
        class_symbol,
        &attribute,
        &record,
        &class,
    );
}

fn assert_pending_defaults(
    attribute: &AttributeDefinitionResult,
    record: &RecordDefinitionResult,
    class: &ClassDefinitionResult,
) {
    let parameters = attribute.definition().expect("attribute").parameters();
    assert_eq!(parameters[0].parameter().raw(), 0);
    assert_eq!(parameters[1].parameter().raw(), 1);
    assert!(parameters[0].pending_default().is_none());
    assert!(parameters[1].default_value().is_none());
    let attribute_pending = parameters[1].pending_default().expect("pending attribute");
    assert_eq!(
        attribute_pending.expected_type(),
        parameters[1].parameter_type()
    );
    assert!(matches!(
        attribute_pending.expression().kind(),
        ExpressionSyntaxKind::Call { .. }
    ));

    let record_field = &record.definition().expect("record").fields()[0];
    let class_field = &class.definition().expect("class").fields()[0];
    assert!(record_field.default().is_none());
    assert!(class_field.default().is_none());
    assert!(record_field.pending_default().is_some());
    assert!(class_field.pending_default().is_some());
}

fn install_and_assert_defaults(
    resolver: &mut SignatureResolver<'_>,
    attribute_symbol: SymbolId,
    record_symbol: SymbolId,
    class_symbol: SymbolId,
    attribute: &AttributeDefinitionResult,
    record: &RecordDefinitionResult,
    class: &ClassDefinitionResult,
) {
    let parameters = attribute.definition().expect("attribute").parameters();
    let record_field = &record.definition().expect("record").fields()[0];
    let class_field = &class.definition().expect("class").fields()[0];
    let wrong = resolver
        .install_record_field_default(
            record_symbol,
            record_field.field(),
            FieldDefault::Boolean(true),
        )
        .expect_err("Boolean is not UInt8");
    assert_eq!(
        wrong,
        RequiredConstantError::TypeMismatch {
            target: RequiredConstantTarget::RecordField {
                definition: record_symbol,
                field: record_field.field(),
            },
            expected: record_field.field_type(),
        }
    );
    assert!(
        resolver
            .record_definition(record_symbol)
            .expect("record")
            .fields()[0]
            .pending_default()
            .is_some()
    );

    let seven = IntegerValue::parse_decimal("7", IntegerKind::UInt8).expect("UInt8");
    resolver
        .install_attribute_parameter_default(
            attribute_symbol,
            parameters[1].parameter(),
            AttributeConstant::Integer(seven),
        )
        .expect("attribute default");
    resolver
        .install_record_field_default(
            record_symbol,
            record_field.field(),
            FieldDefault::Integer(seven),
        )
        .expect("record default");
    resolver
        .install_class_field_default(
            class_symbol,
            class_field.field(),
            FieldDefault::Integer(seven),
        )
        .expect("class default");

    let installed_parameter = &resolver
        .attribute_definition(attribute_symbol)
        .expect("attribute")
        .parameters()[1];
    assert_eq!(
        installed_parameter.default_value(),
        Some(&AttributeConstant::Integer(seven))
    );
    assert!(installed_parameter.pending_default().is_none());
    let installed_record = &resolver
        .record_definition(record_symbol)
        .expect("record")
        .fields()[0];
    assert_eq!(
        installed_record.default(),
        Some(&FieldDefault::Integer(seven))
    );
    assert!(installed_record.pending_default().is_none());
    let installed_class = &resolver
        .class_definition(class_symbol)
        .expect("class")
        .fields()[0];
    assert_eq!(
        installed_class.default(),
        Some(&FieldDefault::Integer(seven))
    );
    assert!(installed_class.pending_default().is_none());
}

#[test]
fn attribute_uses_can_delegate_explicit_arguments_to_a_typed_evaluator() {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/attributeEvaluation.pop",
        "namespace Example\n\
         public attribute Build(value: UInt8)\n\
         @Build(calculate())\n\
         private function run()\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let declaration = parse_attribute_declaration(
        &source,
        &syntax,
        node(&syntax, NodeKind::AttributeDeclaration),
    )
    .expect("attribute declaration");
    let attribute_use =
        parse_attribute_use(&source, &syntax, node(&syntax, NodeKind::AttributeUse))
            .expect("attribute use");
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let attribute_symbol = symbol(&indexed, "Example.Build");
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let definition = resolver.define_attribute_schema(module, attribute_symbol, &declaration);
    let expected = definition.definition().expect("attribute").parameters()[0].parameter_type();

    let mut evaluated = Vec::new();
    let forty_two = IntegerValue::parse_decimal("42", IntegerKind::UInt8).expect("UInt8");
    let result = resolver.resolve_attribute_use_with_evaluator(
        module,
        &attribute_use,
        |expression, expected_type| {
            evaluated.push((expression.span(), expected_type));
            Ok(AttributeConstant::Integer(forty_two))
        },
    );
    assert!(result.diagnostics().is_empty());
    assert_eq!(
        evaluated,
        vec![(attribute_use.arguments()[0].value().span(), expected)]
    );
    assert_eq!(
        result.attribute().expect("attribute").arguments()[0].value(),
        &AttributeConstant::Integer(forty_two)
    );

    let invalid = resolver.resolve_attribute_use_with_evaluator(module, &attribute_use, |_, _| {
        Ok(AttributeConstant::Boolean(true))
    });
    assert!(invalid.attribute().is_none());
    assert_eq!(invalid.diagnostics()[0].code().as_str(), "POP2003");

    let failed =
        resolver.resolve_attribute_use_with_evaluator(module, &attribute_use, |expression, _| {
            Err(vec![
                compile_time_diagnostics::ineligible_constant_expression(
                    expression.span(),
                    "attribute argument",
                ),
            ])
        });
    assert!(failed.attribute().is_none());
    assert_eq!(failed.diagnostics()[0].code().as_str(), "POP4001");
}

fn node(syntax: &pop_syntax::SyntaxTree, kind: NodeKind) -> &pop_syntax::SyntaxNode {
    syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == kind)
        .expect("syntax node")
}

fn symbol(indexed: &pop_resolve::IndexResult, name: &str) -> pop_foundation::SymbolId {
    indexed
        .index()
        .declaration_by_qualified_name(name, SymbolSpace::Type)[0]
        .symbol()
}
