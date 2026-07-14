use std::collections::BTreeSet;

use pop_types::{BootstrapTypeRole, embedded_bootstrap_schema};

#[test]
fn internal_collection_types_have_versioned_semantic_identities() {
    let schema = embedded_bootstrap_schema().expect("bootstrap metadata");
    let bytes = schema.type_by_source_name("Bytes").expect("Bytes");
    let array = schema.type_by_source_name("Array").expect("Array");
    let table = schema.type_by_source_name("Table").expect("Table");

    assert_eq!(bytes.owner_bubble(), "Pop.Internal");
    assert_eq!(bytes.arity(), 0);
    assert_eq!(bytes.role(), BootstrapTypeRole::Nominal);
    assert_eq!(array.arity(), 1);
    assert_eq!(array.role(), BootstrapTypeRole::Array);
    assert_eq!(table.arity(), 2);
    assert_eq!(table.role(), BootstrapTypeRole::Table);
    assert!(bytes.is_in_prelude() && array.is_in_prelude() && table.is_in_prelude());
}

#[test]
fn standard_prelude_types_and_protocols_are_explicit_not_user_injectable() {
    let schema = embedded_bootstrap_schema().expect("bootstrap metadata");
    let result = schema.type_by_source_name("Result").expect("Result");
    let iterable = schema.type_by_source_name("Iterable").expect("Iterable");
    let iterator = schema.type_by_source_name("Iterator").expect("Iterator");
    let iteration = schema.type_by_source_name("Iteration").expect("Iteration");
    let list = schema.type_by_source_name("List").expect("List");
    let protocol = schema.iteration_protocol().expect("iteration protocol");

    assert_eq!(result.owner_bubble(), "Pop.Standard");
    assert_eq!(result.arity(), 2);
    assert_eq!(iterable.role(), BootstrapTypeRole::Interface);
    assert_eq!(iterator.role(), BootstrapTypeRole::Interface);
    assert_eq!(iterable.arity(), 1);
    assert_eq!(iterator.arity(), 1);
    assert_eq!(iteration.id().raw(), 113);
    assert_eq!(iteration.role(), BootstrapTypeRole::Nominal);
    assert_eq!(iteration.arity(), 1);
    assert_eq!(protocol.iteration(), iteration.id());
    assert_eq!(protocol.iterable(), iterable.id());
    assert_eq!(protocol.iterator(), iterator.id());
    assert_eq!(protocol.list(), list.id());
    assert_eq!(protocol.item_case().raw(), 0);
    assert_eq!(protocol.end_case().raw(), 1);
    assert_eq!(protocol.iterator_method().raw(), 0);
    assert_eq!(protocol.next_method().raw(), 1);
}

#[test]
fn foundational_type_ids_and_source_names_are_unique() {
    let schema = embedded_bootstrap_schema().expect("bootstrap metadata");
    let ids: BTreeSet<_> = schema.types().iter().map(|entry| entry.id()).collect();
    let names: BTreeSet<_> = schema
        .types()
        .iter()
        .map(|entry| entry.source_name())
        .collect();

    assert_eq!(ids.len(), schema.types().len());
    assert_eq!(names.len(), schema.types().len());
    assert!(schema.type_by_source_name("Object").is_none());
    assert!(schema.type_by_source_name("Any").is_none());
    assert!(schema.type_by_source_name("Dynamic").is_none());
}
