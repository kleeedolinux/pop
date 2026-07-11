use pop_foundation::{ClassId, InterfaceId};
use pop_types::{
    ClassContract, ClassExtensibility, IntegerKind, IntegerOverflow, PrimitiveType, SemanticType,
};

#[test]
fn numeric_types_have_target_independent_widths_and_aliases() {
    let int = PrimitiveType::from_source_name("Int").expect("Int is built in");
    let byte = PrimitiveType::from_source_name("Byte").expect("Byte is built in");
    let float = PrimitiveType::from_source_name("Float").expect("Float is built in");

    assert_eq!(int, PrimitiveType::Integer(IntegerKind::Int64));
    assert_eq!(byte, PrimitiveType::Integer(IntegerKind::UInt8));
    assert_eq!(float, PrimitiveType::Float64);
    assert_eq!(IntegerKind::Int64.bit_width(), 64);
    assert_eq!(IntegerKind::UInt8.bit_width(), 8);
    assert_eq!(IntegerKind::Int32.default_overflow(), IntegerOverflow::Trap);
}

#[test]
fn primitive_schema_contains_no_dynamic_fallback_type() {
    let names: Vec<_> = PrimitiveType::source_schema()
        .iter()
        .map(|entry| entry.source_name())
        .collect();

    assert!(!names.contains(&"Any"));
    assert!(!names.contains(&"Dynamic"));
    assert_eq!(PrimitiveType::from_source_name("Any"), None);
    assert_eq!(PrimitiveType::from_source_name("Dynamic"), None);
    assert!(!SemanticType::Error.is_valid_hir_type());
}

#[test]
fn class_contracts_are_sealed_single_base_and_nominal_by_default() {
    let contract = ClassContract::new(
        ClassId::from_raw(4),
        Some(ClassId::from_raw(1)),
        vec![InterfaceId::from_raw(8), InterfaceId::from_raw(3)],
    );

    assert_eq!(contract.extensibility(), ClassExtensibility::Sealed);
    assert_eq!(contract.base(), Some(ClassId::from_raw(1)));
    assert_eq!(
        contract.interfaces(),
        &[InterfaceId::from_raw(3), InterfaceId::from_raw(8)]
    );
}
