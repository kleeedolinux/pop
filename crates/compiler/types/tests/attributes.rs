use pop_foundation::{BubbleId, FileId, ModuleId};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{NodeKind, parse_attribute_declaration, parse_attribute_use, parse_file};
use pop_types::{
    AttributeConstant, FloatKind, FloatValue, IntegerKind, IntegerValue, SignatureResolver,
    embedded_bootstrap_schema,
};

fn resolve_attachment(use_text: &str) -> pop_types::ResolvedAttributeResult {
    resolve_attachment_with_parameters("version: UInt32 = 1, label: String = \"default\"", use_text)
}

fn resolve_attachment_with_parameters(
    parameters: &str,
    use_text: &str,
) -> pop_types::ResolvedAttributeResult {
    let module = ModuleId::from_raw(0);
    let text = format!(
        "namespace Example\n\
         public attribute Serializable({parameters})\n\
         {use_text}\n\
         private record User\n\
             name: String\n\
         end\n"
    );
    let source = SourceFile::new(FileId::from_raw(0), "src/user.pop", text).expect("source");
    let syntax = parse_file(&source);
    let declaration_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::AttributeDeclaration)
        .expect("attribute declaration");
    let use_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::AttributeUse)
        .expect("attribute use");
    let declaration = parse_attribute_declaration(&source, &syntax, declaration_node)
        .expect("attribute declaration syntax");
    let attribute_use = parse_attribute_use(&source, &syntax, use_node).expect("attribute use");
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.Serializable", SymbolSpace::Type)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let definition = resolver.define_attribute(module, symbol, &declaration);
    assert!(definition.diagnostics().is_empty());
    resolver.resolve_attribute_use(module, &attribute_use)
}

#[test]
fn named_arguments_are_typed_and_defaults_expand_in_declaration_order() {
    let result = resolve_attachment("@Serializable(version = 2)");

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let attribute = result.attribute().expect("resolved attribute");
    assert_eq!(attribute.arguments().len(), 2);
    assert_eq!(attribute.arguments()[0].name(), "version");
    assert_eq!(
        attribute.arguments()[0].value(),
        &AttributeConstant::Integer(
            IntegerValue::parse_decimal("2", IntegerKind::UInt32).expect("UInt32")
        )
    );
    assert_eq!(attribute.arguments()[1].name(), "label");
    assert_eq!(
        attribute.arguments()[1].value(),
        &AttributeConstant::String("default".to_owned())
    );
}

#[test]
fn positional_arguments_and_type_errors_never_fall_back_to_metadata_strings() {
    let positional = resolve_attachment("@Serializable(3, \"stable\")");
    assert!(positional.diagnostics().is_empty());
    assert_eq!(
        positional.attribute().expect("attribute").arguments()[0].value(),
        &AttributeConstant::Integer(
            IntegerValue::parse_decimal("3", IntegerKind::UInt32).expect("UInt32")
        )
    );

    let mismatch = resolve_attachment("@Serializable(version = \"wrong\")");
    assert!(mismatch.attribute().is_none());
    assert!(mismatch.diagnostic_snapshot().starts_with("POP2003"));
}

#[test]
fn numeric_attribute_constants_preserve_width_signedness_and_float_format() {
    let result = resolve_attachment_with_parameters(
        "minimum: Int8 = -128, maximum: UInt64 = 18446744073709551615, ratio: Float32 = 1, precise: Float64 = 2",
        "@Serializable()",
    );

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let arguments = result.attribute().expect("attribute").arguments();
    assert_eq!(
        arguments[0].value(),
        &AttributeConstant::Integer(
            IntegerValue::parse_decimal("-128", IntegerKind::Int8).expect("Int8")
        )
    );
    assert_eq!(
        arguments[1].value(),
        &AttributeConstant::Integer(
            IntegerValue::parse_decimal("18446744073709551615", IntegerKind::UInt64)
                .expect("UInt64")
        )
    );
    assert_eq!(
        arguments[2].value(),
        &AttributeConstant::Float(
            FloatValue::parse_decimal("1", FloatKind::Float32).expect("Float32")
        )
    );
    assert_eq!(
        arguments[3].value(),
        &AttributeConstant::Float(
            FloatValue::parse_decimal("2", FloatKind::Float64).expect("Float64")
        )
    );
}

#[test]
fn out_of_range_attribute_numbers_are_rejected_before_canonicalization() {
    let result = resolve_attachment_with_parameters("value: UInt8", "@Serializable(256)");

    assert!(result.attribute().is_none());
    assert_eq!(
        result.diagnostics().len(),
        1,
        "{}",
        result.diagnostic_snapshot()
    );
    assert_eq!(result.diagnostics()[0].code().as_str(), "POP4002");
}

#[test]
fn duplicate_and_unknown_named_arguments_are_rejected() {
    for (use_text, expected) in [
        ("@Serializable(version = 1, version = 2)", "POP2011"),
        ("@Serializable(missing = 1)", "POP2012"),
    ] {
        let result = resolve_attachment(use_text);
        assert!(result.attribute().is_none());
        assert!(result.diagnostic_snapshot().starts_with(expected));
    }
}

#[test]
fn resolved_non_attribute_types_are_rejected_instead_of_discarded() {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/user.pop",
        "namespace Example\n\
         public record NotAttribute\n\
         end\n\
         @NotAttribute\n\
         private record User\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let use_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::AttributeUse)
        .expect("attribute use");
    let attribute_use = parse_attribute_use(&source, &syntax, use_node).expect("attribute use");
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let database = ResolutionDatabase::new(indexed.into_index());
    let resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));

    let result = resolver.resolve_attribute_use(module, &attribute_use);

    assert!(result.attribute().is_none());
    assert_eq!(result.diagnostics().len(), 1);
    assert_eq!(result.diagnostics()[0].code().as_str(), "POP4001");
}
