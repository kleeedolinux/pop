use std::collections::BTreeSet;

use pop_foundation::AttributeId;
use pop_types::{
    AttributeIdentity, BootstrapTypeRole, CompilerAttributeRole, CompilerAttributeTarget,
    PrimitiveType, embedded_bootstrap_schema,
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
    assert!(ids.contains("Array.Create"));
    assert!(ids.contains("Array.Get"));
    assert!(ids.contains("Array.GetOptional"));
    assert!(ids.contains("Array.Set"));
    assert!(ids.contains("Array.Fill"));
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
fn standard_print_overloads_have_stable_typed_prelude_identities() {
    let schema = embedded_bootstrap_schema().expect("valid embedded bootstrap schema");
    let print: Vec<_> = schema
        .standard_functions()
        .iter()
        .filter(|entry| entry.source_name() == "print")
        .collect();

    assert_eq!(print.len(), 2);
    assert_eq!(print[0].id().raw(), 0);
    assert_eq!(print[0].parameter_types(), ["Int"]);
    assert_eq!(print[1].id().raw(), 1);
    assert_eq!(print[1].parameter_types(), ["String"]);
    assert!(print.iter().all(|entry| {
        entry.owner_bubble() == "Pop.Standard"
            && entry.result_types().is_empty()
            && entry.effects() == ["AmbientIo"]
            && entry.is_in_prelude()
    }));
    assert!(
        schema
            .standard_functions_by_source_name("Print")
            .next()
            .is_none()
    );
}

#[test]
fn compile_time_attribute_has_a_stable_trusted_prelude_contract() {
    let schema = embedded_bootstrap_schema().expect("valid embedded bootstrap schema");
    let attributes = schema.compiler_attributes();

    assert_eq!(attributes.len(), 7);
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
fn ffi_abi_types_have_stable_qualified_non_prelude_identities() {
    let schema = embedded_bootstrap_schema().expect("valid embedded bootstrap schema");
    let expected = [
        (200, "Ffi.Pointer", 1),
        (201, "Ffi.OptionalPointer", 1),
        (202, "Ffi.Function", 1),
        (203, "Ffi.OptionalFunction", 1),
        (204, "Ffi.Handle", 1),
        (210, "Ffi.C.Char", 0),
        (211, "Ffi.C.SignedChar", 0),
        (212, "Ffi.C.UnsignedChar", 0),
        (213, "Ffi.C.Short", 0),
        (214, "Ffi.C.UnsignedShort", 0),
        (215, "Ffi.C.Int", 0),
        (216, "Ffi.C.UnsignedInt", 0),
        (217, "Ffi.C.Long", 0),
        (218, "Ffi.C.UnsignedLong", 0),
        (219, "Ffi.C.LongLong", 0),
        (220, "Ffi.C.UnsignedLongLong", 0),
        (221, "Ffi.C.Size", 0),
        (222, "Ffi.C.PointerDifference", 0),
    ];

    for (id, source_name, arity) in expected {
        let entry = schema
            .type_by_source_name(source_name)
            .unwrap_or_else(|| panic!("missing trusted {source_name} type"));
        assert_eq!(entry.id().raw(), id);
        assert_eq!(entry.owner_bubble(), "Pop.Ffi");
        assert_eq!(entry.arity(), arity);
        assert_eq!(entry.role(), BootstrapTypeRole::Nominal);
        assert!(!entry.is_in_prelude());
    }

    for unqualified in [
        "Pointer",
        "OptionalPointer",
        "Function",
        "OptionalFunction",
        "Handle",
        "Char",
        "Size",
    ] {
        assert!(schema.type_by_source_name(unqualified).is_none());
    }
}

#[test]
fn ffi_attributes_have_stable_trusted_non_prelude_contracts() {
    let schema = embedded_bootstrap_schema().expect("valid embedded bootstrap schema");
    let expected = [
        (
            CompilerAttributeRole::FfiLink,
            100,
            "Ffi.Link",
            1,
            CompilerAttributeTarget::Namespace,
        ),
        (
            CompilerAttributeRole::FfiForeign,
            101,
            "Ffi.Foreign",
            2,
            CompilerAttributeTarget::Function,
        ),
        (
            CompilerAttributeRole::FfiNonblocking,
            102,
            "Ffi.Nonblocking",
            0,
            CompilerAttributeTarget::Function,
        ),
        (
            CompilerAttributeRole::FfiCLayout,
            103,
            "Ffi.C.Layout",
            0,
            CompilerAttributeTarget::Record,
        ),
    ];

    for (role, id, source_name, argument_count, target) in expected {
        let entry = schema
            .compiler_attribute_by_role(role)
            .unwrap_or_else(|| panic!("missing trusted {source_name} attribute"));
        assert_eq!(entry.id().raw(), id);
        assert_eq!(entry.source_name(), source_name);
        assert_eq!(entry.owner_bubble(), "Pop.Ffi");
        assert_eq!(entry.argument_count(), argument_count);
        assert_eq!(entry.target(), target);
        assert!(!entry.is_in_prelude());
        assert_eq!(
            schema.compiler_attribute_by_source_name(source_name),
            Some(entry)
        );
        assert_eq!(schema.compiler_attribute_role(entry.identity()), Some(role));
    }

    assert!(schema.compiler_attribute_by_source_name("Link").is_none());
    assert!(
        schema
            .compiler_attribute_by_source_name("Foreign")
            .is_none()
    );
    assert!(
        schema
            .compiler_attribute_by_source_name("Nonblocking")
            .is_none()
    );
    assert!(
        schema
            .compiler_attribute_by_source_name("Ffi.C.layout")
            .is_none()
    );
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
