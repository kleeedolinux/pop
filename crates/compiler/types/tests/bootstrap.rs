use std::collections::BTreeSet;

use pop_foundation::AttributeId;
use pop_types::{
    AttributeIdentity, CompilerAttributeRole, CompilerAttributeTarget, PrimitiveType,
    embedded_bootstrap_schema,
};

#[test]
fn embedded_primitive_metadata_matches_the_semantic_type_contract() {
    let schema = embedded_bootstrap_schema().expect("valid embedded bootstrap schema");
    let metadata_names: Vec<_> = schema
        .primitives()
        .iter()
        .map(|entry| entry.source_name())
        .collect();
    let semantic_names: Vec<_> = PrimitiveType::source_schema()
        .iter()
        .map(|entry| entry.source_name())
        .collect();

    assert_eq!(schema.version(), 1);
    assert_eq!(metadata_names, semantic_names);
    assert!(schema.primitives().iter().all(|entry| {
        !entry.source_name().contains("Any") && !entry.source_name().contains("Dynamic")
    }));
}

#[test]
fn intrinsic_ids_are_unique_typed_and_backend_neutral() {
    let schema = embedded_bootstrap_schema().expect("valid embedded bootstrap schema");
    let ids: BTreeSet<_> = schema
        .intrinsics()
        .iter()
        .map(pop_types::BootstrapIntrinsicEntry::intrinsic_id)
        .collect();

    assert_eq!(ids.len(), schema.intrinsics().len());
    assert!(ids.contains("Integer.CheckedAdd"));
    assert!(ids.contains("Array.Length"));
    assert!(ids.contains("Gc.SafePoint"));
    assert!(ids.contains("Gc.SatbWriteBarrier"));
    assert!(ids.contains("Gc.GenerationalWriteBarrier"));
    assert!(schema.intrinsics().iter().all(|entry| {
        let contract = format!(
            "{} {} {}",
            entry.owner(),
            entry.signature(),
            entry.lowering_kind()
        );
        !contract.to_ascii_lowercase().contains("llvm")
            && !contract.contains("Any")
            && !contract.contains("Dynamic")
    }));
}

#[test]
fn standard_print_has_a_stable_typed_prelude_identity() {
    let schema = embedded_bootstrap_schema().expect("valid embedded bootstrap schema");
    let print = schema
        .standard_function_by_source_name("print")
        .expect("trusted print function");

    assert_eq!(print.id().raw(), 0);
    assert_eq!(print.owner_bubble(), "Pop.Standard");
    assert_eq!(print.parameter_types(), ["Int"]);
    assert!(print.result_types().is_empty());
    assert_eq!(print.effects(), ["AmbientIo"]);
    assert!(print.is_in_prelude());
    assert!(schema.standard_function_by_source_name("Print").is_none());
}

#[test]
fn compile_time_attribute_has_a_stable_trusted_prelude_contract() {
    let schema = embedded_bootstrap_schema().expect("valid embedded bootstrap schema");
    let attributes = schema.compiler_attributes();

    assert_eq!(attributes.len(), 3);
    let compile_time = schema
        .compiler_attribute_by_role(CompilerAttributeRole::CompileTime)
        .expect("trusted CompileTime attribute");
    assert_eq!(compile_time.id().raw(), 0);
    assert_eq!(compile_time.source_name(), "CompileTime");
    assert_eq!(compile_time.owner_bubble(), "Pop.Standard");
    assert_eq!(compile_time.argument_count(), 0);
    assert_eq!(compile_time.target(), CompilerAttributeTarget::Function);
    assert!(compile_time.is_in_prelude());
    assert_eq!(
        schema.compiler_attribute_by_source_name("CompileTime"),
        Some(compile_time)
    );
    assert_eq!(
        schema.compiler_attribute_role(compile_time.identity()),
        Some(CompilerAttributeRole::CompileTime)
    );

    let usage = schema
        .compiler_attribute_by_role(CompilerAttributeRole::AttributeUsage)
        .expect("trusted AttributeUsage attribute");
    assert_eq!(usage.id().raw(), 1);
    assert_eq!(usage.source_name(), "AttributeUsage");
    assert_eq!(usage.owner_bubble(), "Pop.Standard");
    assert_eq!(usage.argument_count(), 2);
    assert_eq!(usage.target(), CompilerAttributeTarget::Attribute);
    assert!(usage.is_in_prelude());

    let validator = schema
        .compiler_attribute_by_role(CompilerAttributeRole::AttributeValidator)
        .expect("trusted AttributeValidator attribute");
    assert_eq!(validator.id().raw(), 2);
    assert_eq!(validator.source_name(), "AttributeValidator");
    assert_eq!(validator.owner_bubble(), "Pop.Standard");
    assert_eq!(validator.argument_count(), 1);
    assert_eq!(validator.target(), CompilerAttributeTarget::Attribute);
    assert!(validator.is_in_prelude());
}

#[test]
fn user_attribute_identity_cannot_gain_a_compiler_role_by_reusing_spelling_or_raw_id() {
    let schema = embedded_bootstrap_schema().expect("valid embedded bootstrap schema");
    for name in ["CompileTime", "AttributeUsage", "AttributeValidator"] {
        let compiler_attribute = schema
            .compiler_attribute_by_source_name(name)
            .expect("trusted compiler attribute");
        let user_identity =
            AttributeIdentity::User(AttributeId::from_raw(compiler_attribute.id().raw()));

        assert_ne!(user_identity, compiler_attribute.identity());
        assert_eq!(schema.compiler_attribute_role(user_identity), None);
        let mut wrong_case = name.to_owned();
        wrong_case.replace_range(..1, &name[..1].to_ascii_lowercase());
        assert!(
            schema
                .compiler_attribute_by_source_name(&wrong_case)
                .is_none()
        );
    }
}
