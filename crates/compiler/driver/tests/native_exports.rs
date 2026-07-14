use pop_driver::{NativeExportValidationError, validate_standard_native_exports};
use pop_library_bridge::{FoundationBubble, NativeExport, PopAbiType};
use pop_types::embedded_bootstrap_schema;

#[test]
fn annotated_standard_adapters_match_the_trusted_bootstrap_schema() {
    let schema = embedded_bootstrap_schema().expect("trusted bootstrap schema");
    validate_standard_native_exports(&schema, pop_standard::NATIVE_EXPORTS)
        .expect("typed native exports match bootstrap metadata");
}

#[test]
fn mismatched_or_duplicate_adapter_descriptors_fail_closed() {
    let schema = embedded_bootstrap_schema().expect("trusted bootstrap schema");
    let wrong = NativeExport::new(
        FoundationBubble::Standard,
        "Pop",
        "print",
        "wrong_print",
        &[PopAbiType::UInt64],
        &[],
        &[],
    );
    assert!(matches!(
        validate_standard_native_exports(&schema, &[wrong]),
        Err(NativeExportValidationError::ExportCount {
            expected: 26,
            actual: 1
        })
    ));

    let first = pop_standard::NATIVE_EXPORTS[0];
    let mut duplicate = pop_standard::NATIVE_EXPORTS.to_vec();
    duplicate[1] = first;
    assert!(matches!(
        validate_standard_native_exports(&schema, &duplicate),
        Err(NativeExportValidationError::DuplicateBinding { .. })
    ));
}
