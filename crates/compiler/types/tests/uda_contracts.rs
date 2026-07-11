use pop_foundation::{BubbleId, FileId, FunctionId, ModuleId, SymbolId};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{
    NodeKind, parse_attribute_declaration, parse_attribute_use, parse_file,
    parse_record_declaration,
};
use pop_types::{
    AttributeAttachmentError, AttributeContractError, AttributeQueryError, AttributeQuerySubject,
    AttributeQueryValue, AttributeTarget, AttributeUsage, AttributeValidator, SignatureResolver,
    embedded_bootstrap_schema,
};

struct UdaFixture {
    owner_module: ModuleId,
    consumer_module: ModuleId,
    database: ResolutionDatabase,
    owner_source: SourceFile,
    owner_syntax: pop_syntax::SyntaxTree,
    once: SymbolId,
    many: SymbolId,
    hidden: SymbolId,
    visible: SymbolId,
    secret: SymbolId,
}

fn fixture() -> UdaFixture {
    let owner_module = ModuleId::from_raw(0);
    let consumer_module = ModuleId::from_raw(1);
    let owner_source = SourceFile::new(
        FileId::from_raw(0),
        "src/owner.pop",
        "namespace Owner\n\
         public attribute Once()\n\
         public attribute Many(label: String)\n\
         private attribute Hidden()\n\
         @Many(\"first\")\n\
         @Once()\n\
         @Many(\"second\")\n\
         public record Visible\n\
             name: String\n\
         end\n\
         private record Secret\n\
             value: Int\n\
         end\n",
    )
    .expect("owner source");
    let consumer_source = SourceFile::new(
        FileId::from_raw(1),
        "src/consumer.pop",
        "namespace Consumer\nprivate function inspect()\nend\n",
    )
    .expect("consumer source");
    let owner_syntax = parse_file(&owner_source);
    let consumer_syntax = parse_file(&consumer_source);
    let indexed = build_declaration_index(&[
        ModuleInput::new(
            owner_module,
            BubbleId::from_raw(0),
            &owner_source,
            &owner_syntax,
        ),
        ModuleInput::new(
            consumer_module,
            BubbleId::from_raw(0),
            &consumer_source,
            &consumer_syntax,
        ),
    ]);
    let symbol = |name: &str| {
        indexed
            .index()
            .declaration_by_qualified_name(name, SymbolSpace::Type)[0]
            .symbol()
    };
    let once = symbol("Owner.Once");
    let many = symbol("Owner.Many");
    let hidden = symbol("Owner.Hidden");
    let visible = symbol("Owner.Visible");
    let secret = symbol("Owner.Secret");
    UdaFixture {
        owner_module,
        consumer_module,
        database: ResolutionDatabase::new(indexed.into_index()),
        owner_source,
        owner_syntax,
        once,
        many,
        hidden,
        visible,
        secret,
    }
}

fn define_attributes(fixture: &UdaFixture) -> SignatureResolver<'_> {
    let mut resolver = SignatureResolver::new(
        &fixture.database,
        embedded_bootstrap_schema().expect("bootstrap"),
    );
    for node in fixture
        .owner_syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == NodeKind::AttributeDeclaration)
    {
        let declaration =
            parse_attribute_declaration(&fixture.owner_source, &fixture.owner_syntax, node)
                .expect("attribute declaration");
        let symbol = match declaration.name() {
            "Once" => fixture.once,
            "Many" => fixture.many,
            "Hidden" => fixture.hidden,
            other => panic!("unexpected attribute {other}"),
        };
        let result = resolver.define_attribute(fixture.owner_module, symbol, &declaration);
        assert!(
            result.diagnostics().is_empty(),
            "{}",
            result.diagnostic_snapshot()
        );
    }
    for node in fixture
        .owner_syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == NodeKind::RecordDeclaration)
    {
        let declaration =
            parse_record_declaration(&fixture.owner_source, &fixture.owner_syntax, node)
                .expect("record declaration");
        let symbol = match declaration.name() {
            "Visible" => fixture.visible,
            "Secret" => fixture.secret,
            other => panic!("unexpected record {other}"),
        };
        let result = resolver.define_record(fixture.owner_module, symbol, &declaration);
        assert!(
            result.diagnostics().is_empty(),
            "{}",
            result.diagnostic_snapshot()
        );
    }
    resolver
}

fn resolve_source_attachments(
    fixture: &UdaFixture,
    resolver: &SignatureResolver<'_>,
) -> Vec<pop_types::ResolvedAttribute> {
    fixture
        .owner_syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == NodeKind::AttributeUse)
        .map(|node| {
            let syntax = parse_attribute_use(&fixture.owner_source, &fixture.owner_syntax, node)
                .expect("attribute use");
            let result = resolver.resolve_attribute_use(fixture.owner_module, &syntax);
            assert!(
                result.diagnostics().is_empty(),
                "{}",
                result.diagnostic_snapshot()
            );
            result.attribute().expect("resolved attribute").clone()
        })
        .collect()
}

#[test]
fn omitted_usage_is_namespace_only_and_non_repeatable() {
    let usage = AttributeUsage::default();

    assert_eq!(usage.targets(), &[AttributeTarget::Namespace]);
    assert!(usage.allows(AttributeTarget::Namespace));
    assert!(!usage.allows(AttributeTarget::Record));
    assert!(!usage.allows(AttributeTarget::Field));
    assert!(!usage.is_repeatable());

    let explicit = AttributeUsage::new(
        [
            AttributeTarget::Field,
            AttributeTarget::Record,
            AttributeTarget::Field,
        ],
        true,
    );
    assert_eq!(
        explicit.targets(),
        &[AttributeTarget::Record, AttributeTarget::Field]
    );
    assert!(explicit.is_repeatable());
    assert_eq!(
        AttributeTarget::all(),
        &[
            AttributeTarget::Namespace,
            AttributeTarget::Function,
            AttributeTarget::Constant,
            AttributeTarget::TypeAlias,
            AttributeTarget::Attribute,
            AttributeTarget::Record,
            AttributeTarget::Union,
            AttributeTarget::Class,
            AttributeTarget::Interface,
            AttributeTarget::Enum,
            AttributeTarget::Field,
            AttributeTarget::Case,
            AttributeTarget::Method,
        ]
    );
}

#[test]
fn usage_and_validator_contracts_are_installed_by_resolved_identity_once() {
    let fixture = fixture();
    let mut resolver = define_attributes(&fixture);
    let definition = resolver
        .attribute_definition(fixture.once)
        .expect("Once definition");
    assert_eq!(definition.usage(), &AttributeUsage::default());
    assert!(!definition.has_explicit_usage());
    assert_eq!(definition.validator(), None);

    resolver
        .install_attribute_usage(
            fixture.once,
            AttributeUsage::new([AttributeTarget::Record], false),
        )
        .expect("usage");
    let validator = AttributeValidator::new(FunctionId::from_raw(7));
    resolver
        .install_attribute_validator(fixture.once, validator)
        .expect("validator");
    let definition = resolver
        .attribute_definition(fixture.once)
        .expect("Once definition");
    assert!(definition.has_explicit_usage());
    assert!(definition.usage().allows(AttributeTarget::Record));
    assert_eq!(definition.validator(), Some(validator));
    assert_eq!(validator.function(), FunctionId::from_raw(7));

    assert_eq!(
        resolver.install_attribute_usage(fixture.once, AttributeUsage::default()),
        Err(AttributeContractError::UsageAlreadySpecified {
            definition: fixture.once,
        })
    );
    assert_eq!(
        resolver.install_attribute_validator(
            fixture.once,
            AttributeValidator::new(FunctionId::from_raw(8)),
        ),
        Err(AttributeContractError::ValidatorAlreadySpecified {
            definition: fixture.once,
            original: FunctionId::from_raw(7),
        })
    );
}

#[test]
fn attachment_validation_rejects_wrong_targets_and_non_repeatable_duplicates() {
    let fixture = fixture();
    let mut resolver = define_attributes(&fixture);
    let attachments = resolve_source_attachments(&fixture, &resolver);

    let wrong_target = resolver
        .validate_attribute_attachments(AttributeTarget::Record, attachments.iter().cloned());
    assert!(wrong_target.attachment_set().is_none());
    assert_eq!(wrong_target.errors().len(), attachments.len());
    assert!(wrong_target.errors().iter().all(|error| matches!(
        error,
        AttributeAttachmentError::WrongTarget {
            target: AttributeTarget::Record,
            ..
        }
    )));

    resolver
        .install_attribute_usage(
            fixture.once,
            AttributeUsage::new([AttributeTarget::Record], false),
        )
        .expect("Once usage");
    resolver
        .install_attribute_usage(
            fixture.many,
            AttributeUsage::new([AttributeTarget::Record], true),
        )
        .expect("Many usage");
    let mut duplicated = attachments.clone();
    duplicated.push(attachments[1].clone());
    let duplicate = resolver.validate_attribute_attachments(AttributeTarget::Record, duplicated);
    assert!(duplicate.attachment_set().is_none());
    assert_eq!(duplicate.errors().len(), 1);
    assert!(matches!(
        duplicate.errors()[0],
        AttributeAttachmentError::NonRepeatableDuplicate { .. }
    ));
}

#[test]
fn every_closed_attribute_target_is_validated_by_identity() {
    for target in AttributeTarget::all() {
        let fixture = fixture();
        let mut resolver = define_attributes(&fixture);
        resolver
            .install_attribute_usage(fixture.once, AttributeUsage::new([*target], false))
            .expect("target usage");
        let once = resolve_source_attachments(&fixture, &resolver)
            .into_iter()
            .find(|attribute| {
                attribute.attribute()
                    == resolver
                        .attribute_definition(fixture.once)
                        .expect("Once definition")
                        .attribute()
            })
            .expect("Once attachment");
        let validated = resolver.validate_attribute_attachments(*target, [once]);

        assert!(validated.errors().is_empty(), "{target:?}");
        assert_eq!(
            validated
                .attachment_set()
                .expect("valid attachment")
                .attachments()
                .len(),
            1,
            "{target:?}"
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn valid_attachments_and_repeatable_queries_preserve_source_order() {
    let fixture = fixture();
    let mut resolver = define_attributes(&fixture);
    resolver
        .install_attribute_usage(
            fixture.once,
            AttributeUsage::new([AttributeTarget::Record], false),
        )
        .expect("Once usage");
    resolver
        .install_attribute_usage(
            fixture.many,
            AttributeUsage::new([AttributeTarget::Record], true),
        )
        .expect("Many usage");
    let attachments = resolve_source_attachments(&fixture, &resolver);
    let validated =
        resolver.validate_attribute_attachments(AttributeTarget::Record, attachments.clone());
    assert!(validated.errors().is_empty());
    let set = validated.attachment_set().expect("attachment set");
    let source_order: Vec<_> = set
        .attachments()
        .iter()
        .map(pop_types::ResolvedAttribute::attribute)
        .collect();
    assert_eq!(
        source_order,
        vec![
            resolver
                .attribute_definition(fixture.many)
                .unwrap()
                .attribute(),
            resolver
                .attribute_definition(fixture.once)
                .unwrap()
                .attribute(),
            resolver
                .attribute_definition(fixture.many)
                .unwrap()
                .attribute(),
        ]
    );

    let many_id = resolver
        .attribute_definition(fixture.many)
        .unwrap()
        .attribute();
    let once_id = resolver
        .attribute_definition(fixture.once)
        .unwrap()
        .attribute();
    let hidden_id = resolver
        .attribute_definition(fixture.hidden)
        .unwrap()
        .attribute();
    let visible_type = resolver
        .record_definition(fixture.visible)
        .expect("Visible definition")
        .type_id();
    let empty = resolver
        .validate_attribute_attachments(AttributeTarget::Record, std::iter::empty())
        .attachment_set()
        .expect("empty attachment set")
        .clone();
    let mut query_index = resolver.attribute_query_index();
    query_index
        .insert_symbol(fixture.visible, set.clone())
        .expect("symbol attachments");
    query_index
        .insert_type(visible_type, fixture.visible, set.clone())
        .expect("type attachments");
    query_index
        .insert_symbol(fixture.secret, empty)
        .expect("secret symbol attachments");
    let _arena = resolver.into_arena();

    match query_index
        .attribute(
            fixture.owner_module,
            AttributeQuerySubject::Symbol(fixture.visible),
            once_id,
        )
        .expect("optional query")
    {
        AttributeQueryValue::Optional(Some(attribute)) => {
            assert_eq!(attribute.attribute(), once_id);
        }
        AttributeQueryValue::Optional(None) => panic!("missing non-repeatable attribute"),
        AttributeQueryValue::ImmutableSequence(_) => panic!("unexpected repeatable result"),
    }
    assert!(matches!(
        query_index
            .attribute(
                fixture.owner_module,
                AttributeQuerySubject::Type(visible_type),
                once_id,
            )
            .expect("resolved type query"),
        AttributeQueryValue::Optional(Some(_))
    ));
    match query_index
        .attribute(
            fixture.owner_module,
            AttributeQuerySubject::Symbol(fixture.visible),
            many_id,
        )
        .expect("sequence query")
    {
        AttributeQueryValue::ImmutableSequence(attributes) => {
            let labels: Vec<_> = attributes
                .iter()
                .map(|attribute| attribute.arguments()[0].value())
                .collect();
            assert_eq!(
                labels,
                vec![
                    &pop_types::AttributeConstant::String("first".to_owned()),
                    &pop_types::AttributeConstant::String("second".to_owned()),
                ]
            );
        }
        AttributeQueryValue::Optional(other) => {
            panic!("unexpected optional query result: {other:?}")
        }
    }
    assert!(
        query_index
            .has_attribute(
                fixture.owner_module,
                AttributeQuerySubject::Symbol(fixture.visible),
                many_id,
            )
            .expect("has attribute")
    );
    assert!(matches!(
        query_index
            .attribute(
                fixture.owner_module,
                AttributeQuerySubject::Symbol(fixture.visible),
                hidden_id,
            )
            .expect("known absent attribute"),
        AttributeQueryValue::Optional(None)
    ));

    assert_eq!(
        query_index.attribute(
            fixture.consumer_module,
            AttributeQuerySubject::Symbol(fixture.visible),
            hidden_id,
        ),
        Err(AttributeQueryError::InaccessibleAttribute {
            attribute: hidden_id,
            definition: fixture.hidden,
        })
    );
    assert_eq!(
        query_index.attribute(
            fixture.consumer_module,
            AttributeQuerySubject::Symbol(fixture.secret),
            once_id,
        ),
        Err(AttributeQueryError::InaccessibleSubject {
            subject: AttributeQuerySubject::Symbol(fixture.secret),
            definition: fixture.secret,
        })
    );
}
