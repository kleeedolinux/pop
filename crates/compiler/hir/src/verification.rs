//! Independent verification for typed backend-neutral HIR.
//!
//! Verification is kept out of construction so every producer and future
//! transformation can prove the same invariants before MIR lowering. Errors
//! are deterministic and never repaired with dynamic fallback behavior.

use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{
    BindingId, BuiltinTypeId, CaptureId, ClassId, EnumCaseId, ErrorCaseId, ErrorId, FieldId,
    InterfaceId, InterfaceMethodId, IterationCaseId, LocalId, MethodId, NestedFunctionId,
    NominalInterfaceId, ResultCaseId, SourceSpan, SymbolId, SymbolIdentity, TypeId, UnionCaseId,
    ValueParameterId,
};
use pop_resolve::Visibility;
use pop_types::{
    ClassMethodDispatch, FloatKind, NumericConversionKind, PrimitiveType, SemanticType, TypeArena,
    TypedBinaryOperator, TypedUnaryOperator, embedded_bootstrap_schema,
};

use crate::ir::*;

fn ffi_handle_payload(arena: &TypeArena, type_id: TypeId) -> Option<TypeId> {
    match arena.get(type_id)? {
        SemanticType::Builtin {
            definition,
            arguments,
        } if *definition == pop_types::FFI_HANDLE_TYPE_ID && arguments.len() == 1 => {
            Some(arguments[0])
        }
        _ => None,
    }
}

fn ffi_buffer_payload(arena: &TypeArena, type_id: TypeId) -> Option<TypeId> {
    match arena.get(type_id)? {
        SemanticType::Builtin {
            definition,
            arguments,
        } if *definition == pop_types::FFI_BUFFER_TYPE_ID && arguments.len() == 1 => {
            Some(arguments[0])
        }
        _ => None,
    }
}

fn ffi_exact_pointer_payload(
    arena: &TypeArena,
    type_id: TypeId,
    expected: BuiltinTypeId,
) -> Option<TypeId> {
    match arena.get(type_id)? {
        SemanticType::Builtin {
            definition,
            arguments,
        } if *definition == expected && arguments.len() == 1 => Some(arguments[0]),
        _ => None,
    }
}

fn is_ffi_buffer_element(arena: &TypeArena, type_id: TypeId, ffi_c_layout: bool) -> bool {
    match arena.get(type_id) {
        Some(SemanticType::Primitive(
            PrimitiveType::Integer(_) | PrimitiveType::Float32 | PrimitiveType::Float64,
        )) => true,
        Some(SemanticType::Builtin { definition, .. }) => {
            pop_types::is_ffi_integer_abi_builtin_type(*definition)
                || pop_types::is_ffi_pointer_type_constructor(*definition)
                || pop_types::is_ffi_function_type_constructor(*definition)
                || *definition == pop_types::FFI_HANDLE_TYPE_ID
        }
        Some(SemanticType::Record(_)) => ffi_c_layout,
        _ => false,
    }
}

fn valid_ffi_element_metadata(
    arena: &TypeArena,
    schema: Option<&HirSchema>,
    element: TypeId,
    layout_record: Option<SymbolId>,
) -> bool {
    is_ffi_buffer_element(arena, element, layout_record.is_some())
        && layout_record.is_none_or(|record| {
            schema.is_some_and(|schema| {
                schema
                    .records
                    .get(&record)
                    .is_some_and(|record| record.type_id == element && record.ffi_c_layout)
            })
        })
}

fn is_managed_reference_type(arena: &TypeArena, type_id: TypeId) -> bool {
    match arena.get(type_id) {
        Some(SemanticType::Builtin { definition, .. }) => {
            !pop_types::is_ffi_abi_builtin_type(*definition)
        }
        Some(
            SemanticType::Primitive(PrimitiveType::String)
            | SemanticType::Tuple(_)
            | SemanticType::Array(_)
            | SemanticType::Table { .. }
            | SemanticType::Class { .. }
            | SemanticType::Interface { .. }
            | SemanticType::Function { .. }
            | SemanticType::ErrorUnion { .. },
        ) => true,
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HirVerificationError {
    InvalidForeignDeclaration {
        function: SymbolId,
        span: SourceSpan,
    },
    InvalidGenericBounds {
        function: SymbolId,
        span: SourceSpan,
    },
    CompileTimeOnlyExpression {
        span: SourceSpan,
    },
    MissingCanonicalType,
    InvalidType {
        type_id: TypeId,
        span: SourceSpan,
    },
    DuplicateLocal(LocalId),
    DuplicateBinding(BindingId),
    DuplicateCapture(CaptureId),
    DuplicateCapturedBinding(BindingId),
    UnknownCapture {
        capture: CaptureId,
        span: SourceSpan,
    },
    InvalidCaptureSource {
        capture: CaptureId,
        binding: BindingId,
        span: SourceSpan,
    },
    CaptureTypeMismatch {
        capture: CaptureId,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    CaptureModeMismatch {
        capture: CaptureId,
        span: SourceSpan,
    },
    DuplicateNestedFunction(NestedFunctionId),
    DuplicateField(FieldId),
    UnknownLocal {
        local: LocalId,
        span: SourceSpan,
    },
    UnknownParameter {
        parameter: ValueParameterId,
        span: SourceSpan,
    },
    UnknownFunction {
        function: SymbolId,
        span: SourceSpan,
    },
    UnknownReferencedFunction {
        function: SymbolIdentity,
        span: SourceSpan,
    },
    UnknownMethod {
        method: MethodId,
        span: SourceSpan,
    },
    InvalidBuiltinInterfaceCall {
        interface: BuiltinTypeId,
        span: SourceSpan,
    },
    InvalidCollectionType {
        type_id: TypeId,
        span: SourceSpan,
    },
    InvalidCallableType {
        type_id: TypeId,
        span: SourceSpan,
    },
    AwaitOutsideAsync {
        span: SourceSpan,
    },
    InvalidAwaitTask {
        type_id: TypeId,
        span: SourceSpan,
    },
    InvalidTaskOperation {
        span: SourceSpan,
    },
    InvalidFfiHandleOperation {
        span: SourceSpan,
    },
    InvalidFfiBufferOperation {
        span: SourceSpan,
    },
    InvalidFfiBytesBorrow {
        span: SourceSpan,
    },
    InvalidFfiCallbackOperation {
        span: SourceSpan,
    },
    InvalidFfiPointerOperation {
        span: SourceSpan,
    },
    InvalidFfiUnsafeOperation {
        span: SourceSpan,
    },
    ExpressionTypeMismatch {
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    InvalidUnaryOperator {
        operator: TypedUnaryOperator,
        operand: TypeId,
        result: TypeId,
        span: SourceSpan,
    },
    InvalidBinaryOperator {
        operator: TypedBinaryOperator,
        left: TypeId,
        right: TypeId,
        result: TypeId,
        span: SourceSpan,
    },
    InvalidNumericConversion {
        conversion: NumericConversionKind,
        source: TypeId,
        target: TypeId,
        span: SourceSpan,
    },
    WrongReturnArity {
        expected: usize,
        found: usize,
        span: SourceSpan,
    },
    InvalidFixedPack {
        span: SourceSpan,
    },
    InvalidConditionType {
        found: TypeId,
        span: SourceSpan,
    },
    LoopControlOutsideLoop {
        span: SourceSpan,
    },
    InvalidNumericForType {
        type_id: TypeId,
        span: SourceSpan,
    },
    InvalidIterationProtocol {
        span: SourceSpan,
    },
    InvalidIterationSource {
        type_id: TypeId,
        span: SourceSpan,
    },
    InvalidIterationBindings {
        span: SourceSpan,
    },
    DuplicateSymbol(SymbolId),
    DuplicateClass(ClassId),
    DuplicateInterface(InterfaceId),
    DuplicateInterfaceMethod(InterfaceMethodId),
    DuplicateInterfaceImplementation(InterfaceId),
    DuplicateBuiltinInterfaceImplementation(BuiltinTypeId),
    DuplicateDeclaredField(FieldId),
    DuplicateUnionCase {
        union: SymbolId,
        case: UnionCaseId,
    },
    DuplicateEnumCase {
        enumeration: SymbolId,
        case: EnumCaseId,
    },
    DuplicateError(ErrorId),
    DuplicateErrorCase {
        error: ErrorId,
        case: ErrorCaseId,
    },
    InvalidErrorCase {
        error: ErrorId,
        case: ErrorCaseId,
        span: SourceSpan,
    },
    InvalidResultCase {
        case: ResultCaseId,
        span: SourceSpan,
    },
    InvalidIterationCase {
        case: IterationCaseId,
        span: SourceSpan,
    },
    InvalidResultPropagation {
        span: SourceSpan,
    },
    InvalidCleanupControl {
        span: SourceSpan,
    },
    UnknownEnumCase {
        enumeration: SymbolId,
        case: EnumCaseId,
        span: SourceSpan,
    },
    InvalidDeclarationType {
        symbol: SymbolId,
        type_id: TypeId,
        span: SourceSpan,
    },
    UnknownRecord {
        record: SymbolId,
        span: SourceSpan,
    },
    UnknownClass {
        class: ClassId,
        span: SourceSpan,
    },
    UnknownInterface {
        interface: InterfaceId,
        span: SourceSpan,
    },
    WrongInterfaceType {
        interface: InterfaceId,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    UnknownInterfaceMethod {
        interface: InterfaceId,
        method: InterfaceMethodId,
        span: SourceSpan,
    },
    WrongInterfaceMethodSlot {
        interface: InterfaceId,
        method: InterfaceMethodId,
        expected: u32,
        found: u32,
        span: SourceSpan,
    },
    MissingInterfaceMethodMapping {
        class: ClassId,
        interface: InterfaceId,
        method: InterfaceMethodId,
        span: SourceSpan,
    },
    InterfaceMethodMappingMismatch {
        class: ClassId,
        interface: InterfaceId,
        method: InterfaceMethodId,
        class_method: MethodId,
        span: SourceSpan,
    },
    InvalidBuiltinInterfaceImplementation {
        class: ClassId,
        interface: BuiltinTypeId,
    },
    InvalidInterfaceUpcast {
        interface: NominalInterfaceId,
        source: TypeId,
        target: TypeId,
        span: SourceSpan,
    },
    WrongClassDefinition {
        class: ClassId,
        expected: SymbolId,
        found: SymbolId,
        span: SourceSpan,
    },
    UnknownField {
        field: FieldId,
        span: SourceSpan,
    },
    WrongFieldOwner {
        field: FieldId,
        found: TypeId,
        span: SourceSpan,
    },
    ImmutableFieldSet {
        field: FieldId,
        span: SourceSpan,
    },
    MissingDeclaredField {
        field: FieldId,
        span: SourceSpan,
    },
    UnknownUnion {
        union: SymbolId,
        span: SourceSpan,
    },
    UnknownUnionCase {
        union: SymbolId,
        case: UnionCaseId,
        span: SourceSpan,
    },
    UnionCaseArgumentTypeMismatch {
        union: SymbolId,
        case: UnionCaseId,
        index: usize,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    MatchScrutineeTypeMismatch {
        union: SymbolId,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    DuplicateMatchCase {
        union: SymbolId,
        case: UnionCaseId,
        span: SourceSpan,
    },
    MissingMatchCase {
        union: SymbolId,
        case: UnionCaseId,
        span: SourceSpan,
    },
    ForeignMatchCase {
        expected_union: SymbolId,
        found_union: SymbolId,
        case: UnionCaseId,
        span: SourceSpan,
    },
    MatchPayloadArityMismatch {
        union: SymbolId,
        case: UnionCaseId,
        expected: usize,
        found: usize,
        span: SourceSpan,
    },
    MatchPayloadTypeMismatch {
        union: SymbolId,
        case: UnionCaseId,
        index: usize,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    InvalidIgnoredMatchBinding {
        span: SourceSpan,
    },
    InvalidCallSignature {
        expected_arguments: usize,
        found_arguments: usize,
        expected_results: usize,
        found_results: usize,
        span: SourceSpan,
    },
    CallArgumentTypeMismatch {
        index: usize,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    CallResultTypeMismatch {
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    InvalidFunctionReferenceType {
        function: SymbolId,
        found: TypeId,
        span: SourceSpan,
    },
    InvalidMethodSignature {
        method: MethodId,
        span: SourceSpan,
    },
    MissingMethodBody {
        method: MethodId,
        span: SourceSpan,
    },
}

/// Construction and verification share one closed deterministic failure set.
/// Build-only variants prevent compile-time handles or unresolved interface
/// slots from ever becoming HIR nodes; the remaining variants are independently
/// rechecked whenever a complete Bubble is published.
pub type HirBuildError = HirVerificationError;

#[derive(Clone, Debug, Eq, PartialEq)]
struct HirCallableSignature {
    is_async: bool,
    type_parameters: Vec<TypeId>,
    type_parameter_bounds: Vec<Option<TypeId>>,
    parameters: Vec<TypeId>,
    results: Vec<TypeId>,
}

impl HirCallableSignature {
    fn from_function(function: &HirFunction) -> Self {
        Self {
            is_async: function.is_async,
            type_parameters: function.type_parameters().to_vec(),
            type_parameter_bounds: function.type_parameter_bounds().to_vec(),
            parameters: function
                .parameters()
                .iter()
                .map(HirParameter::type_id)
                .collect(),
            results: function.results().to_vec(),
        }
    }

    fn from_foreign(function: &HirForeignFunction) -> Self {
        Self {
            is_async: false,
            type_parameters: Vec::new(),
            type_parameter_bounds: Vec::new(),
            parameters: function
                .parameters()
                .iter()
                .map(HirParameter::type_id)
                .collect(),
            results: function.results().to_vec(),
        }
    }
}

#[derive(Clone)]
struct HirAggregateSchema {
    type_id: TypeId,
    fields: BTreeMap<FieldId, TypeId>,
    ffi_c_layout: bool,
}

#[derive(Clone)]
struct HirUnionSchema {
    type_id: TypeId,
    cases: BTreeMap<UnionCaseId, Vec<TypeId>>,
}

#[derive(Clone)]
struct HirErrorSchema {
    cases: BTreeMap<ErrorCaseId, Vec<TypeId>>,
}

#[derive(Clone)]
struct HirClassSchema {
    definition: SymbolId,
    type_id: TypeId,
    fields: BTreeMap<FieldId, TypeId>,
    interfaces: BTreeMap<InterfaceId, HirInterfaceImplementation>,
    builtin_interfaces: BTreeMap<BuiltinTypeId, HirBuiltinInterfaceImplementation>,
}

#[derive(Clone)]
struct HirInterfaceSchema {
    type_id: TypeId,
    methods: BTreeMap<InterfaceMethodId, HirInterfaceMethodSchema>,
}

#[derive(Clone)]
struct HirInterfaceMethodSchema {
    slot: u32,
    signature: HirCallableSignature,
    span: SourceSpan,
}

#[derive(Clone)]
struct HirDeclaredField {
    owners: BTreeSet<TypeId>,
    field_type: TypeId,
    mutable: bool,
}

struct HirDeclaredMethod {
    class: ClassId,
    definition: SymbolId,
    signature: HirCallableSignature,
    visibility: Visibility,
    dispatch: ClassMethodDispatch,
    span: SourceSpan,
}

struct HirSchema {
    functions: BTreeMap<SymbolId, HirCallableSignature>,
    function_references: BTreeMap<SymbolIdentity, HirCallableSignature>,
    methods: BTreeMap<MethodId, HirCallableSignature>,
    declared_methods: BTreeMap<MethodId, HirDeclaredMethod>,
    records: BTreeMap<SymbolId, HirAggregateSchema>,
    unions: BTreeMap<SymbolId, HirUnionSchema>,
    errors: BTreeMap<ErrorId, HirErrorSchema>,
    enums: BTreeMap<SymbolId, (TypeId, BTreeMap<EnumCaseId, u32>)>,
    classes: BTreeMap<ClassId, HirClassSchema>,
    interfaces: BTreeMap<InterfaceId, HirInterfaceSchema>,
    interface_methods: BTreeMap<InterfaceMethodId, InterfaceId>,
    fields: BTreeMap<FieldId, HirDeclaredField>,
}

impl HirSchema {
    fn collect(
        bubble: &HirBubble,
        arena: &TypeArena,
        errors: &mut Vec<HirVerificationError>,
    ) -> Self {
        let mut schema = Self {
            functions: BTreeMap::new(),
            function_references: BTreeMap::new(),
            methods: BTreeMap::new(),
            declared_methods: BTreeMap::new(),
            records: BTreeMap::new(),
            unions: BTreeMap::new(),
            errors: BTreeMap::new(),
            enums: BTreeMap::new(),
            classes: BTreeMap::new(),
            interfaces: BTreeMap::new(),
            interface_methods: BTreeMap::new(),
            fields: BTreeMap::new(),
        };
        let mut symbols = BTreeSet::new();
        for declaration in bubble.declarations() {
            if !symbols.insert(declaration.symbol()) {
                errors.push(HirVerificationError::DuplicateSymbol(declaration.symbol()));
            }
            schema.collect_declaration(declaration, arena, errors);
        }
        for function in bubble.functions() {
            if !symbols.insert(function.symbol()) {
                errors.push(HirVerificationError::DuplicateSymbol(function.symbol()));
            }
            schema.functions.insert(
                function.symbol(),
                HirCallableSignature::from_function(function),
            );
        }
        for function in bubble.foreign_functions() {
            if !symbols.insert(function.symbol()) {
                errors.push(HirVerificationError::DuplicateSymbol(function.symbol()));
            }
            let declaration = function.declaration();
            let valid_identity = declaration.symbol() == function.symbol()
                && !declaration.external_symbol().is_empty()
                && !declaration.external_symbol().chars().any(char::is_control)
                && declaration.has_valid_effects()
                && declaration.has_valid_callback_pairs()
                && (declaration.callback_pairs().is_empty()
                    || function.visibility() == pop_resolve::Visibility::Internal);
            if !valid_identity {
                errors.push(HirVerificationError::InvalidForeignDeclaration {
                    function: function.symbol(),
                    span: declaration.span(),
                });
            }
            for parameter in function.parameters() {
                verify_schema_type(arena, parameter.type_id(), parameter.span(), errors);
            }
            for result in function.results() {
                verify_schema_type(arena, *result, declaration.span(), errors);
            }
            for attribute in function.attributes() {
                for argument in attribute.arguments() {
                    verify_schema_type(arena, argument.value_type(), argument.origin(), errors);
                }
            }
            schema.functions.insert(
                function.symbol(),
                HirCallableSignature::from_foreign(function),
            );
        }
        for reference in bubble.function_references() {
            for type_id in reference.parameters().iter().chain(reference.results()) {
                verify_schema_type(arena, *type_id, empty_span(), errors);
            }
            if let Some(declaration) = reference.foreign_declaration() {
                let valid = !reference.is_async()
                    && reference.type_parameters().is_empty()
                    && declaration.symbol() == reference.identity().symbol()
                    && declaration.has_valid_effects()
                    && declaration.has_valid_callback_pairs()
                    && declaration.callback_pairs().is_empty()
                    && declaration.effects() == reference.effects()
                    && !declaration.external_symbol().is_empty()
                    && !declaration.external_symbol().chars().any(char::is_control);
                if !valid {
                    errors.push(HirVerificationError::InvalidForeignDeclaration {
                        function: reference.identity().symbol(),
                        span: declaration.span(),
                    });
                }
            }
            schema.function_references.insert(
                reference.identity(),
                HirCallableSignature {
                    is_async: reference.is_async(),
                    type_parameters: reference.type_parameters().to_vec(),
                    type_parameter_bounds: reference.type_parameter_bounds().to_vec(),
                    parameters: reference.parameters().to_vec(),
                    results: reference.results().to_vec(),
                },
            );
        }
        schema.verify_class_interfaces(arena, errors);
        schema.collect_method_bodies(bubble.methods(), errors);
        schema
    }

    #[allow(clippy::too_many_lines)]
    fn collect_declaration(
        &mut self,
        declaration: &HirDeclaration,
        arena: &TypeArena,
        errors: &mut Vec<HirVerificationError>,
    ) {
        match declaration.kind() {
            HirDeclarationKind::Record(record) => {
                for field in &record.fields {
                    verify_schema_type(arena, field.field_type, field.span, errors);
                }
                let semantic_fields: Vec<_> = record
                    .fields
                    .iter()
                    .map(|field| (field.name.clone(), field.field_type))
                    .collect();
                if arena.get(record.type_id) != Some(&SemanticType::Record(semantic_fields)) {
                    errors.push(HirVerificationError::InvalidDeclarationType {
                        symbol: declaration.symbol(),
                        type_id: record.type_id,
                        span: declaration.span(),
                    });
                }
                let fields = self.collect_fields(record.type_id, &record.fields, false, errors);
                self.records.insert(
                    declaration.symbol(),
                    HirAggregateSchema {
                        type_id: record.type_id,
                        fields,
                        ffi_c_layout: record.ffi_c_layout,
                    },
                );
            }
            HirDeclarationKind::Union(union) => {
                if !matches!(
                    arena.get(union.type_id),
                    Some(SemanticType::TaggedUnion { .. })
                ) {
                    errors.push(HirVerificationError::InvalidDeclarationType {
                        symbol: declaration.symbol(),
                        type_id: union.type_id,
                        span: declaration.span(),
                    });
                }
                let mut cases = BTreeMap::new();
                for case in &union.cases {
                    for parameter in &case.parameters {
                        verify_schema_type(arena, parameter.type_id, parameter.span, errors);
                    }
                    let parameters = case.parameters.iter().map(HirNamedType::type_id).collect();
                    if cases.insert(case.case, parameters).is_some() {
                        errors.push(HirVerificationError::DuplicateUnionCase {
                            union: declaration.symbol(),
                            case: case.case,
                        });
                    }
                }
                self.unions.insert(
                    declaration.symbol(),
                    HirUnionSchema {
                        type_id: union.type_id,
                        cases,
                    },
                );
            }
            HirDeclarationKind::Error(error) => {
                if !matches!(
                    arena.get(error.type_id),
                    Some(SemanticType::ErrorUnion {
                        definition,
                        source,
                        ..
                    }) if *definition == error.error && *source == declaration.symbol()
                ) {
                    errors.push(HirVerificationError::InvalidDeclarationType {
                        symbol: declaration.symbol(),
                        type_id: error.type_id,
                        span: declaration.span(),
                    });
                }
                let mut cases = BTreeMap::new();
                for case in &error.cases {
                    for parameter in &case.parameters {
                        verify_schema_type(arena, parameter.type_id, parameter.span, errors);
                    }
                    let parameters = case.parameters.iter().map(HirNamedType::type_id).collect();
                    if cases.insert(case.case, parameters).is_some() {
                        errors.push(HirVerificationError::DuplicateErrorCase {
                            error: error.error,
                            case: case.case,
                        });
                    }
                }
                if self
                    .errors
                    .insert(error.error, HirErrorSchema { cases })
                    .is_some()
                {
                    errors.push(HirVerificationError::DuplicateError(error.error));
                }
            }
            HirDeclarationKind::Enum(enumeration) => {
                if arena.get(enumeration.type_id)
                    != Some(&SemanticType::Enum {
                        definition: declaration.symbol(),
                    })
                {
                    errors.push(HirVerificationError::InvalidDeclarationType {
                        symbol: declaration.symbol(),
                        type_id: enumeration.type_id,
                        span: declaration.span(),
                    });
                }
                let mut cases = BTreeMap::new();
                for case in &enumeration.cases {
                    if cases.insert(case.case, case.discriminant).is_some() {
                        errors.push(HirVerificationError::DuplicateEnumCase {
                            enumeration: declaration.symbol(),
                            case: case.case,
                        });
                    }
                }
                self.enums
                    .insert(declaration.symbol(), (enumeration.type_id, cases));
            }
            HirDeclarationKind::Class(class) => {
                for field in &class.fields {
                    verify_schema_type(arena, field.field_type, field.span, errors);
                }
                for method in &class.methods {
                    for parameter in &method.parameters {
                        verify_schema_type(arena, parameter.type_id, parameter.span, errors);
                    }
                    for result in &method.results {
                        verify_schema_type(arena, *result, method.span, errors);
                    }
                }
                if !matches!(
                    arena.get(class.type_id),
                    Some(SemanticType::Class { class: identity, .. })
                        if *identity == class.class
                ) {
                    errors.push(HirVerificationError::InvalidDeclarationType {
                        symbol: declaration.symbol(),
                        type_id: class.type_id,
                        span: declaration.span(),
                    });
                }
                let fields = self.collect_class_fields(class, errors);
                let mut interfaces = BTreeMap::new();
                for implementation in &class.interfaces {
                    if interfaces
                        .insert(implementation.interface, implementation.clone())
                        .is_some()
                    {
                        errors.push(HirVerificationError::DuplicateInterfaceImplementation(
                            implementation.interface,
                        ));
                    }
                }
                let mut builtin_interfaces = BTreeMap::new();
                for implementation in &class.builtin_interfaces {
                    if builtin_interfaces
                        .insert(implementation.interface, implementation.clone())
                        .is_some()
                    {
                        errors.push(
                            HirVerificationError::DuplicateBuiltinInterfaceImplementation(
                                implementation.interface,
                            ),
                        );
                    }
                }
                if self
                    .classes
                    .insert(
                        class.class,
                        HirClassSchema {
                            definition: declaration.symbol(),
                            type_id: class.type_id,
                            fields,
                            interfaces,
                            builtin_interfaces,
                        },
                    )
                    .is_some()
                {
                    errors.push(HirVerificationError::DuplicateClass(class.class));
                }
                self.collect_declared_methods(declaration.symbol(), class, errors);
            }
            HirDeclarationKind::Interface(interface) => {
                if !matches!(
                    arena.get(interface.type_id),
                    Some(SemanticType::Interface { interface: identity, .. })
                        if *identity == interface.interface
                ) {
                    errors.push(HirVerificationError::InvalidDeclarationType {
                        symbol: declaration.symbol(),
                        type_id: interface.type_id,
                        span: declaration.span(),
                    });
                }
                let mut methods = BTreeMap::new();
                for (expected_slot, method) in interface.methods.iter().enumerate() {
                    for parameter in &method.parameters {
                        verify_schema_type(arena, parameter.type_id, parameter.span, errors);
                    }
                    for result in &method.results {
                        verify_schema_type(arena, *result, method.span, errors);
                    }
                    if method.slot != u32::try_from(expected_slot).unwrap_or(u32::MAX) {
                        errors.push(HirVerificationError::WrongInterfaceMethodSlot {
                            interface: interface.interface,
                            method: method.method,
                            expected: u32::try_from(expected_slot).unwrap_or(u32::MAX),
                            found: method.slot,
                            span: method.span,
                        });
                    }
                    if self
                        .interface_methods
                        .insert(method.method, interface.interface)
                        .is_some()
                        || methods
                            .insert(
                                method.method,
                                HirInterfaceMethodSchema {
                                    slot: method.slot,
                                    signature: HirCallableSignature {
                                        is_async: false,
                                        type_parameters: Vec::new(),
                                        type_parameter_bounds: Vec::new(),
                                        parameters: method
                                            .parameters
                                            .iter()
                                            .map(HirNamedType::type_id)
                                            .collect(),
                                        results: method.results.clone(),
                                    },
                                    span: method.span,
                                },
                            )
                            .is_some()
                    {
                        errors.push(HirVerificationError::DuplicateInterfaceMethod(
                            method.method,
                        ));
                    }
                }
                if self
                    .interfaces
                    .insert(
                        interface.interface,
                        HirInterfaceSchema {
                            type_id: interface.type_id,
                            methods,
                        },
                    )
                    .is_some()
                {
                    errors.push(HirVerificationError::DuplicateInterface(
                        interface.interface,
                    ));
                }
            }
            HirDeclarationKind::Attribute(attribute) => {
                for parameter in &attribute.parameters {
                    if !arena.is_valid_hir_type(parameter.parameter_type) {
                        errors.push(HirVerificationError::InvalidType {
                            type_id: parameter.parameter_type,
                            span: parameter.span,
                        });
                    }
                }
            }
        }
    }

    fn collect_fields(
        &mut self,
        owner: TypeId,
        fields: &[HirRecordField],
        mutable: bool,
        errors: &mut Vec<HirVerificationError>,
    ) -> BTreeMap<FieldId, TypeId> {
        let mut declared = BTreeMap::new();
        for field in fields {
            self.collect_field(owner, field.field, field.field_type, mutable, errors);
            if declared.insert(field.field, field.field_type).is_some() {
                errors.push(HirVerificationError::DuplicateDeclaredField(field.field));
            }
        }
        declared
    }

    fn collect_class_fields(
        &mut self,
        class: &HirClassDeclaration,
        errors: &mut Vec<HirVerificationError>,
    ) -> BTreeMap<FieldId, TypeId> {
        let mut declared = BTreeMap::new();
        for field in &class.fields {
            self.collect_field(class.type_id, field.field, field.field_type, true, errors);
            if declared.insert(field.field, field.field_type).is_some() {
                errors.push(HirVerificationError::DuplicateDeclaredField(field.field));
            }
        }
        declared
    }

    fn collect_field(
        &mut self,
        owner: TypeId,
        field: FieldId,
        field_type: TypeId,
        mutable: bool,
        errors: &mut Vec<HirVerificationError>,
    ) {
        if let Some(existing) = self.fields.get_mut(&field) {
            if existing.field_type != field_type || existing.mutable != mutable {
                errors.push(HirVerificationError::DuplicateDeclaredField(field));
            } else {
                existing.owners.insert(owner);
            }
            return;
        }
        self.fields.insert(
            field,
            HirDeclaredField {
                owners: BTreeSet::from([owner]),
                field_type,
                mutable,
            },
        );
    }

    fn collect_declared_methods(
        &mut self,
        definition: SymbolId,
        class: &HirClassDeclaration,
        errors: &mut Vec<HirVerificationError>,
    ) {
        for method in &class.methods {
            let mut parameters = Vec::new();
            if method.dispatch == ClassMethodDispatch::Receiver {
                parameters.push(class.type_id);
            }
            parameters.extend(method.parameters.iter().map(HirNamedType::type_id));
            let declared = HirDeclaredMethod {
                class: class.class,
                definition,
                signature: HirCallableSignature {
                    is_async: false,
                    type_parameters: Vec::new(),
                    type_parameter_bounds: Vec::new(),
                    parameters,
                    results: method.results.clone(),
                },
                visibility: method.visibility,
                dispatch: method.dispatch,
                span: method.span,
            };
            if self
                .methods
                .insert(method.method, declared.signature.clone())
                .is_some()
            {
                errors.push(HirVerificationError::UnknownMethod {
                    method: method.method,
                    span: method.span,
                });
            }
            self.declared_methods.insert(method.method, declared);
        }
    }

    fn verify_class_interfaces(&self, arena: &TypeArena, errors: &mut Vec<HirVerificationError>) {
        let protocol = embedded_bootstrap_schema()
            .ok()
            .and_then(|schema| schema.iteration_protocol());
        for (class_id, class) in &self.classes {
            let mut seen = BTreeSet::new();
            for implementation in class.interfaces.values() {
                if !seen.insert(implementation.interface) {
                    errors.push(HirVerificationError::DuplicateInterfaceImplementation(
                        implementation.interface,
                    ));
                }
                let Some(interface) = self.interfaces.get(&implementation.interface) else {
                    errors.push(HirVerificationError::UnknownInterface {
                        interface: implementation.interface,
                        span: empty_span(),
                    });
                    continue;
                };
                if implementation.interface_type != interface.type_id {
                    errors.push(HirVerificationError::WrongInterfaceType {
                        interface: implementation.interface,
                        expected: interface.type_id,
                        found: implementation.interface_type,
                        span: empty_span(),
                    });
                }
                let mut mapped = BTreeSet::new();
                for mapping in &implementation.methods {
                    if !mapped.insert(mapping.interface_method) {
                        errors.push(HirVerificationError::DuplicateInterfaceMethod(
                            mapping.interface_method,
                        ));
                        continue;
                    }
                    let Some(required) = interface.methods.get(&mapping.interface_method) else {
                        errors.push(HirVerificationError::UnknownInterfaceMethod {
                            interface: implementation.interface,
                            method: mapping.interface_method,
                            span: empty_span(),
                        });
                        continue;
                    };
                    if mapping.slot != required.slot {
                        errors.push(HirVerificationError::WrongInterfaceMethodSlot {
                            interface: implementation.interface,
                            method: mapping.interface_method,
                            expected: required.slot,
                            found: mapping.slot,
                            span: required.span,
                        });
                    }
                    let valid_class_method = self
                        .declared_methods
                        .get(&mapping.class_method)
                        .is_some_and(|method| {
                            method.class == *class_id
                                && method.visibility == Visibility::Public
                                && method.dispatch == ClassMethodDispatch::Receiver
                                && method.signature.parameters.first() == Some(&class.type_id)
                                && method.signature.parameters[1..] == required.signature.parameters
                                && method.signature.results == required.signature.results
                        });
                    if !valid_class_method {
                        errors.push(HirVerificationError::InterfaceMethodMappingMismatch {
                            class: *class_id,
                            interface: implementation.interface,
                            method: mapping.interface_method,
                            class_method: mapping.class_method,
                            span: required.span,
                        });
                    }
                }
                for required in interface.methods.keys() {
                    if !mapped.contains(required) {
                        errors.push(HirVerificationError::MissingInterfaceMethodMapping {
                            class: *class_id,
                            interface: implementation.interface,
                            method: *required,
                            span: interface.methods[required].span,
                        });
                    }
                }
            }
            let Some(protocol) = protocol else {
                continue;
            };
            for implementation in class.builtin_interfaces.values() {
                let item_type = match arena.get(implementation.interface_type) {
                    Some(SemanticType::Builtin {
                        definition,
                        arguments,
                    }) if *definition == implementation.interface && arguments.len() == 1 => {
                        arguments[0]
                    }
                    _ => {
                        errors.push(
                            HirVerificationError::InvalidBuiltinInterfaceImplementation {
                                class: *class_id,
                                interface: implementation.interface,
                            },
                        );
                        continue;
                    }
                };
                let expected_methods = if implementation.interface == protocol.iterable() {
                    vec![protocol.iterator_method()]
                } else if implementation.interface == protocol.iterator() {
                    vec![protocol.iterator_method(), protocol.next_method()]
                } else {
                    Vec::new()
                };
                let iterator_type = arena.find(&SemanticType::Builtin {
                    definition: protocol.iterator(),
                    arguments: vec![item_type],
                });
                let iteration_type = arena.find(&SemanticType::Builtin {
                    definition: protocol.iteration(),
                    arguments: vec![item_type],
                });
                let mut mapped = BTreeSet::new();
                let valid = !expected_methods.is_empty()
                    && implementation.methods.len() == expected_methods.len()
                    && implementation.methods.iter().all(|mapping| {
                        let expected_result =
                            if mapping.protocol_method == protocol.iterator_method() {
                                iterator_type
                            } else if mapping.protocol_method == protocol.next_method()
                                && implementation.interface == protocol.iterator()
                            {
                                iteration_type
                            } else {
                                None
                            };
                        mapped.insert(mapping.protocol_method)
                            && expected_methods.contains(&mapping.protocol_method)
                            && expected_result.is_some_and(|expected_result| {
                                self.declared_methods
                                    .get(&mapping.class_method)
                                    .is_some_and(|method| {
                                        method.class == *class_id
                                            && method.visibility == Visibility::Public
                                            && method.dispatch == ClassMethodDispatch::Receiver
                                            && method.signature.parameters.as_slice()
                                                == [class.type_id]
                                            && method.signature.results.as_slice()
                                                == [expected_result]
                                    })
                            })
                    })
                    && expected_methods
                        .iter()
                        .all(|method| mapped.contains(method));
                if !valid {
                    errors.push(
                        HirVerificationError::InvalidBuiltinInterfaceImplementation {
                            class: *class_id,
                            interface: implementation.interface,
                        },
                    );
                }
            }
        }
    }

    fn collect_method_bodies(
        &mut self,
        methods: &[HirMethod],
        errors: &mut Vec<HirVerificationError>,
    ) {
        let mut bodies = BTreeSet::new();
        for method in methods {
            bodies.insert(method.method());
            let span = method_span(method);
            let Some(declared) = self.declared_methods.get(&method.method()) else {
                errors.push(HirVerificationError::UnknownMethod {
                    method: method.method(),
                    span,
                });
                continue;
            };
            if method.class() != declared.class {
                errors.push(HirVerificationError::UnknownClass {
                    class: method.class(),
                    span,
                });
            }
            if method.definition() != declared.definition {
                errors.push(HirVerificationError::WrongClassDefinition {
                    class: method.class(),
                    expected: declared.definition,
                    found: method.definition(),
                    span,
                });
            }
            if HirCallableSignature::from_function(method.function()) != declared.signature {
                errors.push(HirVerificationError::InvalidMethodSignature {
                    method: method.method(),
                    span,
                });
            }
        }
        for (method, declared) in &self.declared_methods {
            if !bodies.contains(method) {
                errors.push(HirVerificationError::MissingMethodBody {
                    method: *method,
                    span: declared.span,
                });
            }
        }
    }
}

fn verify_schema_type(
    arena: &TypeArena,
    type_id: TypeId,
    span: SourceSpan,
    errors: &mut Vec<HirVerificationError>,
) {
    if !arena.is_valid_hir_type(type_id) {
        errors.push(HirVerificationError::InvalidType { type_id, span });
    }
}

/// Verifies a complete backend-neutral HIR Bubble, including declaration and
/// member schemas plus exact direct, method, and indirect callable signatures.
///
/// # Errors
///
/// Returns invariant violations in deterministic declaration and body order.
pub fn verify_hir_bubble(
    bubble: &HirBubble,
    arena: &TypeArena,
) -> Result<(), Vec<HirVerificationError>> {
    let mut errors = Vec::new();
    let schema = HirSchema::collect(bubble, arena, &mut errors);
    let known_functions: BTreeSet<_> = schema.functions.keys().copied().collect();
    let known_methods: BTreeSet<_> = schema.methods.keys().copied().collect();
    for function in bubble.functions() {
        if let Err(mut function_errors) = verify_hir_callable_with_schema(
            function,
            arena,
            &known_functions,
            &known_methods,
            Some(&schema),
        ) {
            errors.append(&mut function_errors);
        }
    }
    for method in bubble.methods() {
        if let Err(mut method_errors) = verify_hir_callable_with_schema(
            method.function(),
            arena,
            &known_functions,
            &known_methods,
            Some(&schema),
        ) {
            errors.append(&mut method_errors);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Verifies backend-neutral HIR typing, local identity, and dispatch targets.
///
/// # Errors
///
/// Returns invariant violations in deterministic traversal order.
pub fn verify_hir_function(
    function: &HirFunction,
    arena: &TypeArena,
    known_functions: &BTreeSet<SymbolId>,
) -> Result<(), Vec<HirVerificationError>> {
    verify_hir_callable(function, arena, known_functions, &BTreeSet::new())
}

pub(crate) fn verify_hir_callable(
    function: &HirFunction,
    arena: &TypeArena,
    known_functions: &BTreeSet<SymbolId>,
    known_methods: &BTreeSet<MethodId>,
) -> Result<(), Vec<HirVerificationError>> {
    verify_hir_callable_with_schema(function, arena, known_functions, known_methods, None)
}

fn verify_hir_callable_with_schema(
    function: &HirFunction,
    arena: &TypeArena,
    known_functions: &BTreeSet<SymbolId>,
    known_methods: &BTreeSet<MethodId>,
    schema: Option<&HirSchema>,
) -> Result<(), Vec<HirVerificationError>> {
    if function.type_parameters.len() != function.type_parameter_names.len()
        || function.type_parameters.len() != function.type_parameter_bounds.len()
        || function
            .type_parameters
            .iter()
            .any(|parameter| !matches!(arena.get(*parameter), Some(SemanticType::TypeParameter(_))))
        || function
            .type_parameter_bounds
            .iter()
            .flatten()
            .any(|bound| !arena.is_valid_hir_type(*bound))
    {
        return Err(vec![HirVerificationError::InvalidGenericBounds {
            function: function.symbol,
            span: function
                .parameters
                .first()
                .map_or_else(empty_span, |parameter| parameter.span),
        }]);
    }
    let parameter_bindings: Vec<_> = function
        .parameters
        .iter()
        .map(|parameter| parameter.binding)
        .collect();
    let cell_bindings = collect_cell_bindings(
        &function.body,
        &parameter_bindings,
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    let mut verifier = Verifier {
        arena,
        known_functions,
        known_methods,
        schema,
        parameter_bounds: function
            .type_parameters
            .iter()
            .copied()
            .zip(function.type_parameter_bounds.iter().copied())
            .filter_map(|(parameter, bound)| bound.map(|bound| (parameter, bound)))
            .collect(),
        parameter_types: function
            .parameters
            .iter()
            .map(|parameter| parameter.type_id)
            .collect(),
        parameter_bindings,
        results: function.results.clone(),
        local_types: BTreeMap::new(),
        local_bindings: BTreeMap::new(),
        capture_types: BTreeMap::new(),
        capture_bindings: BTreeMap::new(),
        capture_modes: BTreeMap::new(),
        bindings: BTreeSet::new(),
        nested_functions: BTreeSet::new(),
        cell_bindings,
        loop_depth: 0,
        cleanup_depth: 0,
        async_cleanup_depth: 0,
        is_async: function.is_async,
        errors: Vec::new(),
    };
    for parameter in &function.parameters {
        verifier.verify_type(parameter.type_id, parameter.span);
        if !verifier.bindings.insert(parameter.binding) {
            verifier
                .errors
                .push(HirVerificationError::DuplicateBinding(parameter.binding));
        }
    }
    for result in &function.results {
        if !arena.is_valid_hir_type(*result) {
            verifier.errors.push(HirVerificationError::InvalidType {
                type_id: *result,
                span: function
                    .parameters
                    .first()
                    .map_or_else(empty_span, |parameter| parameter.span),
            });
        }
    }
    for attribute in &function.attributes {
        for argument in attribute.arguments() {
            verifier.verify_type(argument.value_type(), argument.origin());
        }
    }
    verifier.verify_statements(&function.body, &BTreeSet::new());
    if verifier.errors.is_empty() {
        Ok(())
    } else {
        Err(verifier.errors)
    }
}

struct Verifier<'arena> {
    arena: &'arena TypeArena,
    known_functions: &'arena BTreeSet<SymbolId>,
    known_methods: &'arena BTreeSet<MethodId>,
    schema: Option<&'arena HirSchema>,
    parameter_bounds: BTreeMap<TypeId, TypeId>,
    parameter_types: Vec<TypeId>,
    parameter_bindings: Vec<BindingId>,
    results: Vec<TypeId>,
    local_types: BTreeMap<LocalId, TypeId>,
    local_bindings: BTreeMap<LocalId, BindingId>,
    capture_types: BTreeMap<CaptureId, TypeId>,
    capture_bindings: BTreeMap<CaptureId, BindingId>,
    capture_modes: BTreeMap<CaptureId, HirCaptureMode>,
    bindings: BTreeSet<BindingId>,
    nested_functions: BTreeSet<NestedFunctionId>,
    cell_bindings: BTreeSet<BindingId>,
    loop_depth: u32,
    cleanup_depth: u32,
    async_cleanup_depth: u32,
    is_async: bool,
    errors: Vec<HirVerificationError>,
}

impl Verifier<'_> {
    fn builtin_type_arguments(&self, type_id: TypeId, name: &str) -> Option<&[TypeId]> {
        let definition = embedded_bootstrap_schema()
            .ok()?
            .type_by_source_name(name)?
            .id();
        match self.arena.get(type_id)? {
            SemanticType::Builtin {
                definition: found,
                arguments,
            } if *found == definition => Some(arguments),
            _ => None,
        }
    }

    fn is_builtin_type(&self, type_id: TypeId, name: &str, arguments: &[TypeId]) -> bool {
        embedded_bootstrap_schema()
            .ok()
            .and_then(|schema| schema.type_by_source_name(name).copied())
            .is_some_and(|entry| {
                matches!(
                    self.arena.get(type_id),
                    Some(SemanticType::Builtin {
                        definition,
                        arguments: found,
                    }) if *definition == entry.id() && found == arguments
                )
            })
    }

    #[allow(clippy::too_many_lines)]
    fn verify_statements(&mut self, statements: &[HirStatement], visible: &BTreeSet<LocalId>) {
        let mut visible = visible.clone();
        for statement in statements {
            match statement.kind() {
                HirStatementKind::Local {
                    binding,
                    local,
                    local_type,
                    initializer,
                    ..
                } => {
                    self.verify_type(*local_type, statement.span());
                    let recursive_closure = matches!(initializer.kind(), HirExpressionKind::Closure(closure)
                    if closure.captures.iter().any(|capture| {
                        capture.binding == *binding
                            && capture.source == HirCaptureSource::Local(*local)
                    }));
                    let mut initializer_visible = visible.clone();
                    if recursive_closure {
                        initializer_visible.insert(*local);
                    }
                    if self.local_types.insert(*local, *local_type).is_some() {
                        self.errors
                            .push(HirVerificationError::DuplicateLocal(*local));
                    }
                    self.local_bindings.insert(*local, *binding);
                    if !self.bindings.insert(*binding) {
                        self.errors
                            .push(HirVerificationError::DuplicateBinding(*binding));
                    }
                    self.verify_expression(initializer, &initializer_visible);
                    self.verify_expression_type(*local_type, initializer);
                    visible.insert(*local);
                }
                HirStatementKind::MultipleLocal { bindings, value } => {
                    self.verify_expression(value, &visible);
                    let element_types = match self.arena.get(value.type_id()) {
                        Some(SemanticType::Tuple(elements)) if elements.len() == bindings.len() => {
                            Some(elements.clone())
                        }
                        _ => {
                            self.errors.push(HirVerificationError::InvalidFixedPack {
                                span: statement.span(),
                            });
                            None
                        }
                    };
                    for (index, binding) in bindings.iter().enumerate() {
                        self.verify_type(binding.local_type, binding.span);
                        if element_types
                            .as_ref()
                            .and_then(|elements| elements.get(index))
                            .is_some_and(|element| *element != binding.local_type)
                        {
                            self.errors.push(HirVerificationError::InvalidFixedPack {
                                span: binding.span,
                            });
                        }
                        if self
                            .local_types
                            .insert(binding.local, binding.local_type)
                            .is_some()
                        {
                            self.errors
                                .push(HirVerificationError::DuplicateLocal(binding.local));
                        }
                        self.local_bindings.insert(binding.local, binding.binding);
                        if !self.bindings.insert(binding.binding) {
                            self.errors
                                .push(HirVerificationError::DuplicateBinding(binding.binding));
                        }
                    }
                    visible.extend(bindings.iter().map(|binding| binding.local));
                }
                HirStatementKind::LocalSet { local, value } => {
                    self.verify_expression(value, &visible);
                    if !visible.contains(local) {
                        self.errors.push(HirVerificationError::UnknownLocal {
                            local: *local,
                            span: statement.span(),
                        });
                    } else if let Some(expected) = self.local_types.get(local).copied() {
                        self.verify_expression_type(expected, value);
                    }
                }
                HirStatementKind::ParameterSet { parameter, value } => {
                    self.verify_expression(value, &visible);
                    if let Some(expected) = self.parameter_type(*parameter) {
                        self.verify_expression_type(expected, value);
                    } else {
                        self.errors.push(HirVerificationError::UnknownParameter {
                            parameter: *parameter,
                            span: statement.span(),
                        });
                    }
                }
                HirStatementKind::CaptureSet { capture, value } => {
                    self.verify_expression(value, &visible);
                    if let Some(expected) = self.capture_types.get(capture).copied() {
                        self.verify_expression_type(expected, value);
                        if self.capture_modes.get(capture) != Some(&HirCaptureMode::Cell) {
                            self.errors.push(HirVerificationError::CaptureModeMismatch {
                                capture: *capture,
                                span: statement.span(),
                            });
                        }
                    } else {
                        self.errors.push(HirVerificationError::UnknownCapture {
                            capture: *capture,
                            span: statement.span(),
                        });
                    }
                }
                HirStatementKind::Return { values } => {
                    if self.cleanup_depth != 0 {
                        self.errors
                            .push(HirVerificationError::InvalidCleanupControl {
                                span: statement.span(),
                            });
                    }
                    for value in values {
                        self.verify_expression(value, &visible);
                    }
                    self.verify_return(values, statement.span());
                }
                HirStatementKind::If {
                    condition,
                    then_body,
                    else_body,
                } => {
                    self.verify_expression(condition, &visible);
                    self.verify_condition(condition);
                    self.verify_statements(then_body, &visible);
                    self.verify_statements(else_body, &visible);
                }
                HirStatementKind::OptionalIf {
                    binding,
                    local,
                    inner_type,
                    initializer,
                    then_body,
                    else_body,
                    ..
                } => {
                    self.verify_expression(initializer, &visible);
                    self.verify_optional_binding(
                        *binding,
                        *local,
                        *inner_type,
                        initializer,
                        statement.span(),
                    );
                    let mut then_visible = visible.clone();
                    then_visible.insert(*local);
                    self.verify_statements(then_body, &then_visible);
                    self.verify_statements(else_body, &visible);
                }
                HirStatementKind::While { condition, body } => {
                    self.verify_expression(condition, &visible);
                    self.verify_condition(condition);
                    self.loop_depth = self.loop_depth.saturating_add(1);
                    self.verify_statements(body, &visible);
                    self.loop_depth = self.loop_depth.saturating_sub(1);
                }
                HirStatementKind::OptionalWhile {
                    binding,
                    local,
                    inner_type,
                    initializer,
                    body,
                    ..
                } => {
                    self.verify_expression(initializer, &visible);
                    self.verify_optional_binding(
                        *binding,
                        *local,
                        *inner_type,
                        initializer,
                        statement.span(),
                    );
                    let mut body_visible = visible.clone();
                    body_visible.insert(*local);
                    self.loop_depth = self.loop_depth.saturating_add(1);
                    self.verify_statements(body, &body_visible);
                    self.loop_depth = self.loop_depth.saturating_sub(1);
                }
                HirStatementKind::RepeatUntil { body, condition } => {
                    self.loop_depth = self.loop_depth.saturating_add(1);
                    self.verify_statements(body, &visible);
                    self.loop_depth = self.loop_depth.saturating_sub(1);
                    let mut condition_visible = visible.clone();
                    for nested in body {
                        if let HirStatementKind::Local { local, .. } = nested.kind() {
                            condition_visible.insert(*local);
                        }
                    }
                    self.verify_expression(condition, &condition_visible);
                    self.verify_condition(condition);
                }
                HirStatementKind::NumericFor {
                    binding,
                    local,
                    integer_type,
                    first,
                    last,
                    step,
                    body,
                    ..
                } => {
                    self.verify_type(*integer_type, statement.span());
                    if !matches!(
                        self.arena.get(*integer_type),
                        Some(SemanticType::Primitive(PrimitiveType::Integer(_)))
                    ) {
                        self.errors
                            .push(HirVerificationError::InvalidNumericForType {
                                type_id: *integer_type,
                                span: statement.span(),
                            });
                    }
                    for expression in [first, last, step] {
                        self.verify_expression(expression, &visible);
                        self.verify_expression_type(*integer_type, expression);
                    }
                    if self.local_types.insert(*local, *integer_type).is_some() {
                        self.errors
                            .push(HirVerificationError::DuplicateLocal(*local));
                    }
                    self.local_bindings.insert(*local, *binding);
                    if !self.bindings.insert(*binding) {
                        self.errors
                            .push(HirVerificationError::DuplicateBinding(*binding));
                    }
                    let mut body_visible = visible.clone();
                    body_visible.insert(*local);
                    self.loop_depth = self.loop_depth.saturating_add(1);
                    self.verify_statements(body, &body_visible);
                    self.loop_depth = self.loop_depth.saturating_sub(1);
                }
                HirStatementKind::GeneralizedFor {
                    protocol,
                    source,
                    item_type,
                    iterator_type,
                    iteration_type,
                    bindings,
                    iterable,
                    body,
                } => {
                    self.verify_type(*item_type, statement.span());
                    self.verify_type(*iterator_type, statement.span());
                    self.verify_type(*iteration_type, statement.span());
                    self.verify_expression(iterable, &visible);
                    let protocol_identities = [
                        protocol.iteration(),
                        protocol.iterable(),
                        protocol.iterator(),
                        protocol.list(),
                        protocol.range(),
                    ];
                    let unique_protocol_identities: BTreeSet<_> =
                        protocol_identities.into_iter().collect();
                    if unique_protocol_identities.len() != protocol_identities.len()
                        || protocol.item_case().raw() != 0
                        || protocol.end_case().raw() != 1
                        || protocol.iterator_method().raw() != 0
                        || protocol.next_method().raw() != 1
                    {
                        self.errors
                            .push(HirVerificationError::InvalidIterationProtocol {
                                span: statement.span(),
                            });
                    }
                    let valid_source = match (source, self.arena.get(iterable.type_id())) {
                        (HirIterationSource::Array, Some(SemanticType::Array(element))) => {
                            *element == *item_type
                        }
                        (HirIterationSource::Table, Some(SemanticType::Table { key, value })) => {
                            matches!(
                                self.arena.get(*item_type),
                                Some(SemanticType::Tuple(elements))
                                    if elements.as_slice() == [*key, *value]
                            )
                        }
                        (
                            HirIterationSource::List,
                            Some(SemanticType::Builtin {
                                definition,
                                arguments,
                            }),
                        ) => *definition == protocol.list() && arguments.as_slice() == [*item_type],
                        (
                            HirIterationSource::Range,
                            Some(SemanticType::Builtin {
                                definition,
                                arguments,
                            }),
                        ) => {
                            *definition == protocol.range()
                                && arguments.as_slice() == [*item_type]
                                && matches!(
                                    self.arena.get(*item_type),
                                    Some(SemanticType::Primitive(
                                        pop_types::PrimitiveType::Integer(_)
                                    ))
                                )
                        }
                        (
                            HirIterationSource::Iterable,
                            Some(SemanticType::Builtin {
                                definition,
                                arguments,
                            }),
                        ) => {
                            *definition == protocol.iterable()
                                && arguments.as_slice() == [*item_type]
                        }
                        (
                            HirIterationSource::Iterator,
                            Some(SemanticType::Builtin {
                                definition,
                                arguments,
                            }),
                        ) => {
                            *definition == protocol.iterator()
                                && arguments.as_slice() == [*item_type]
                        }
                        (
                            HirIterationSource::BoundIterable,
                            Some(SemanticType::TypeParameter(_)),
                        ) => self
                            .parameter_bounds
                            .get(&iterable.type_id())
                            .is_some_and(|bound| {
                                matches!(
                                    self.arena.get(*bound),
                                    Some(SemanticType::Builtin { definition, arguments })
                                        if *definition == protocol.iterable()
                                            && arguments.as_slice() == [*item_type]
                                )
                            }),
                        (
                            HirIterationSource::BoundIterator,
                            Some(SemanticType::TypeParameter(_)),
                        ) => self
                            .parameter_bounds
                            .get(&iterable.type_id())
                            .is_some_and(|bound| {
                                matches!(
                                self.arena.get(*bound),
                                Some(SemanticType::Builtin { definition, arguments })
                                    if *definition == protocol.iterator()
                                        && arguments.as_slice() == [*item_type]
                                )
                            }),
                        (
                            HirIterationSource::ClassIterable { iterator_method },
                            Some(SemanticType::Class { class, .. }),
                        ) => self.schema.map_or_else(
                            || self.known_methods.contains(iterator_method),
                            |schema| {
                                schema.classes.get(class).is_some_and(|class| {
                                    class
                                        .builtin_interfaces
                                        .get(&protocol.iterable())
                                        .is_some_and(|implementation| {
                                            matches!(
                                                self.arena.get(implementation.interface_type),
                                                Some(SemanticType::Builtin { arguments, .. })
                                                    if arguments.as_slice() == [*item_type]
                                            ) && implementation.methods.iter().any(|method| {
                                                method.protocol_method == protocol.iterator_method()
                                                    && method.class_method == *iterator_method
                                            })
                                        })
                                })
                            },
                        ),
                        (
                            HirIterationSource::ClassIterator {
                                iterator_method,
                                next_method,
                            },
                            Some(SemanticType::Class { class, .. }),
                        ) => self.schema.map_or_else(
                            || {
                                self.known_methods.contains(iterator_method)
                                    && self.known_methods.contains(next_method)
                            },
                            |schema| {
                                schema.classes.get(class).is_some_and(|class| {
                                    class
                                        .builtin_interfaces
                                        .get(&protocol.iterator())
                                        .is_some_and(|implementation| {
                                            matches!(
                                                self.arena.get(implementation.interface_type),
                                                Some(SemanticType::Builtin { arguments, .. })
                                                    if arguments.as_slice() == [*item_type]
                                            ) && implementation.methods.iter().any(|method| {
                                                method.protocol_method == protocol.iterator_method()
                                                    && method.class_method == *iterator_method
                                            }) && implementation.methods.iter().any(|method| {
                                                method.protocol_method == protocol.next_method()
                                                    && method.class_method == *next_method
                                            })
                                        })
                                })
                            },
                        ),
                        _ => false,
                    };
                    if !valid_source {
                        self.errors
                            .push(HirVerificationError::InvalidIterationSource {
                                type_id: iterable.type_id(),
                                span: statement.span(),
                            });
                    }
                    for (type_id, definition) in [
                        (*iterator_type, protocol.iterator()),
                        (*iteration_type, protocol.iteration()),
                    ] {
                        if !matches!(
                            self.arena.get(type_id),
                            Some(SemanticType::Builtin {
                                definition: actual,
                                arguments,
                            }) if *actual == definition && arguments.as_slice() == [*item_type]
                        ) {
                            self.errors
                                .push(HirVerificationError::InvalidIterationProtocol {
                                    span: statement.span(),
                                });
                        }
                    }
                    let binding_types_valid = if bindings.len() == 1 {
                        bindings[0].local_type == *item_type
                    } else {
                        matches!(
                            self.arena.get(*item_type),
                            Some(SemanticType::Tuple(elements))
                                if elements.len() == bindings.len()
                                    && elements.iter().zip(bindings).all(|(element, binding)| {
                                        *element == binding.local_type
                                    })
                        )
                    };
                    if bindings.is_empty() || !binding_types_valid {
                        self.errors
                            .push(HirVerificationError::InvalidIterationBindings {
                                span: statement.span(),
                            });
                    }
                    let mut body_visible = visible.clone();
                    for binding in bindings {
                        self.verify_type(binding.local_type, binding.span);
                        if self
                            .local_types
                            .insert(binding.local, binding.local_type)
                            .is_some()
                        {
                            self.errors
                                .push(HirVerificationError::DuplicateLocal(binding.local));
                        }
                        self.local_bindings.insert(binding.local, binding.binding);
                        if !self.bindings.insert(binding.binding) {
                            self.errors
                                .push(HirVerificationError::DuplicateBinding(binding.binding));
                        }
                        body_visible.insert(binding.local);
                    }
                    self.loop_depth = self.loop_depth.saturating_add(1);
                    self.verify_statements(body, &body_visible);
                    self.loop_depth = self.loop_depth.saturating_sub(1);
                }
                HirStatementKind::Break | HirStatementKind::Continue => {
                    if self.cleanup_depth != 0 {
                        self.errors
                            .push(HirVerificationError::InvalidCleanupControl {
                                span: statement.span(),
                            });
                    }
                    if self.loop_depth == 0 {
                        self.errors
                            .push(HirVerificationError::LoopControlOutsideLoop {
                                span: statement.span(),
                            });
                    }
                }
                HirStatementKind::Match {
                    scrutinee,
                    union,
                    arms,
                } => self.verify_match(scrutinee, *union, arms, statement.span(), &visible),
                HirStatementKind::ErrorMatch {
                    scrutinee,
                    error,
                    arms,
                } => self.verify_error_match(scrutinee, *error, arms, statement.span(), &visible),
                HirStatementKind::ResultMatch {
                    scrutinee,
                    result,
                    result_type,
                    arms,
                } => self.verify_result_match(
                    scrutinee,
                    *result,
                    *result_type,
                    arms,
                    statement.span(),
                    &visible,
                ),
                HirStatementKind::Defer { body } => {
                    if self.cleanup_depth != 0 {
                        self.errors
                            .push(HirVerificationError::InvalidCleanupControl {
                                span: statement.span(),
                            });
                    }
                    self.cleanup_depth = self.cleanup_depth.saturating_add(1);
                    self.verify_statements(body, &visible);
                    self.cleanup_depth = self.cleanup_depth.saturating_sub(1);
                }
                HirStatementKind::AsyncDefer { body } => {
                    if self.cleanup_depth != 0 || !self.is_async {
                        self.errors
                            .push(HirVerificationError::InvalidCleanupControl {
                                span: statement.span(),
                            });
                    }
                    self.cleanup_depth = self.cleanup_depth.saturating_add(1);
                    self.async_cleanup_depth = self.async_cleanup_depth.saturating_add(1);
                    self.verify_statements(body, &visible);
                    self.async_cleanup_depth = self.async_cleanup_depth.saturating_sub(1);
                    self.cleanup_depth = self.cleanup_depth.saturating_sub(1);
                }
                HirStatementKind::FieldSet { base, field, value } => {
                    self.verify_expression(base, &visible);
                    self.verify_expression(value, &visible);
                    self.verify_field_set(*field, base, value, statement.span());
                }
                HirStatementKind::CompoundFieldSet {
                    base,
                    field,
                    value_type,
                    operator,
                    value,
                } => {
                    self.verify_expression(base, &visible);
                    self.verify_expression(value, &visible);
                    self.verify_expression_type(*value_type, value);
                    self.verify_field_set(*field, base, value, statement.span());
                    self.verify_compound_operator(*operator, *value_type, statement.span());
                }
                HirStatementKind::ArraySet {
                    array,
                    index,
                    value,
                } => {
                    self.verify_array_get(array, index, &visible);
                    self.verify_expression(value, &visible);
                    if let Some(SemanticType::Array(element)) = self.arena.get(array.type_id()) {
                        self.verify_expression_type(*element, value);
                    }
                }
                HirStatementKind::ListSet { list, index, value } => {
                    let element = self.verify_list_get(list, index, &visible);
                    self.verify_expression(value, &visible);
                    if let Some(element) = element {
                        self.verify_expression_type(element, value);
                    }
                }
                HirStatementKind::TableSet { table, key, value } => {
                    self.verify_expression(table, &visible);
                    self.verify_expression(key, &visible);
                    self.verify_expression(value, &visible);
                    if let Some(SemanticType::Table {
                        key: key_type,
                        value: value_type,
                    }) = self.arena.get(table.type_id())
                    {
                        self.verify_expression_type(*key_type, key);
                        self.verify_expression_type(*value_type, value);
                    } else {
                        self.errors
                            .push(HirVerificationError::InvalidCollectionType {
                                type_id: table.type_id(),
                                span: statement.span(),
                            });
                    }
                }
                HirStatementKind::CompoundArraySet {
                    array,
                    index,
                    element_type,
                    operator,
                    value,
                } => {
                    self.verify_array_get(array, index, &visible);
                    self.verify_expression(value, &visible);
                    self.verify_expression_type(*element_type, value);
                    if let Some(SemanticType::Array(element)) = self.arena.get(array.type_id())
                        && *element != *element_type
                    {
                        self.errors
                            .push(HirVerificationError::InvalidCollectionType {
                                type_id: array.type_id(),
                                span: statement.span(),
                            });
                    }
                    self.verify_compound_operator(*operator, *element_type, statement.span());
                }
                HirStatementKind::MultipleAssignment { targets, value } => {
                    let mut target_types = Vec::with_capacity(targets.len());
                    for target in targets {
                        target_types.push(self.verify_assignment_target(
                            target,
                            &visible,
                            statement.span(),
                        ));
                    }
                    self.verify_expression(value, &visible);
                    match self.arena.get(value.type_id()) {
                        Some(SemanticType::Tuple(elements))
                            if elements.len() == target_types.len()
                                && target_types
                                    .iter()
                                    .zip(elements)
                                    .all(|(target, element)| target == &Some(*element)) => {}
                        _ => self.errors.push(HirVerificationError::InvalidFixedPack {
                            span: statement.span(),
                        }),
                    }
                }
                HirStatementKind::Call(call) => {
                    self.verify_call(
                        call.dispatch(),
                        call.is_async(),
                        call.type_arguments(),
                        call.arguments(),
                        None,
                        call.span(),
                        &visible,
                    );
                }
                HirStatementKind::Expression(expression) => {
                    self.verify_expression(expression, &visible);
                }
            }
        }
    }

    fn verify_assignment_target(
        &mut self,
        target: &HirAssignmentTarget,
        visible: &BTreeSet<LocalId>,
        span: SourceSpan,
    ) -> Option<TypeId> {
        match target {
            HirAssignmentTarget::Local {
                local, value_type, ..
            } => {
                self.verify_type(*value_type, span);
                if !visible.contains(local) {
                    self.errors.push(HirVerificationError::UnknownLocal {
                        local: *local,
                        span,
                    });
                } else if self.local_types.get(local) != Some(value_type) {
                    self.errors
                        .push(HirVerificationError::InvalidFixedPack { span });
                }
                Some(*value_type)
            }
            HirAssignmentTarget::Capture {
                capture,
                value_type,
                ..
            } => {
                self.verify_type(*value_type, span);
                if self.capture_types.get(capture) != Some(value_type)
                    || self.capture_modes.get(capture) != Some(&HirCaptureMode::Cell)
                {
                    self.errors.push(HirVerificationError::CaptureModeMismatch {
                        capture: *capture,
                        span,
                    });
                }
                Some(*value_type)
            }
            HirAssignmentTarget::Field {
                base,
                field,
                value_type,
            } => {
                self.verify_expression(base, visible);
                self.verify_type(*value_type, span);
                if let Some(declared) = self
                    .schema
                    .and_then(|schema| schema.fields.get(field))
                    .cloned()
                {
                    self.verify_field_owner(*field, base, &declared, span);
                    if declared.field_type != *value_type {
                        self.errors
                            .push(HirVerificationError::InvalidFixedPack { span });
                    }
                    if !declared.mutable {
                        self.errors.push(HirVerificationError::ImmutableFieldSet {
                            field: *field,
                            span,
                        });
                    }
                } else if self.schema.is_some() {
                    self.errors.push(HirVerificationError::UnknownField {
                        field: *field,
                        span,
                    });
                }
                Some(*value_type)
            }
            HirAssignmentTarget::Array {
                array,
                index,
                element_type,
            } => {
                self.verify_array_get(array, index, visible);
                self.verify_type(*element_type, span);
                if !matches!(self.arena.get(array.type_id()), Some(SemanticType::Array(element)) if element == element_type)
                {
                    self.errors
                        .push(HirVerificationError::InvalidFixedPack { span });
                }
                Some(*element_type)
            }
            HirAssignmentTarget::List {
                list,
                index,
                element_type,
            } => {
                let actual = self.verify_list_get(list, index, visible);
                self.verify_type(*element_type, span);
                if actual != Some(*element_type) {
                    self.errors
                        .push(HirVerificationError::InvalidFixedPack { span });
                }
                Some(*element_type)
            }
            HirAssignmentTarget::Table {
                table,
                key,
                value_type,
            } => {
                self.verify_expression(table, visible);
                self.verify_expression(key, visible);
                self.verify_type(*value_type, span);
                if let Some(SemanticType::Table {
                    key: key_type,
                    value,
                }) = self.arena.get(table.type_id())
                {
                    self.verify_expression_type(*key_type, key);
                    if value != value_type {
                        self.errors
                            .push(HirVerificationError::InvalidFixedPack { span });
                    }
                } else {
                    self.errors
                        .push(HirVerificationError::InvalidFixedPack { span });
                }
                Some(*value_type)
            }
        }
    }

    fn verify_expression(&mut self, expression: &HirExpression, visible: &BTreeSet<LocalId>) {
        self.verify_type(expression.type_id(), expression.span());
        match expression.kind() {
            HirExpressionKind::Local(local) => {
                if !visible.contains(local) {
                    self.errors.push(HirVerificationError::UnknownLocal {
                        local: *local,
                        span: expression.span(),
                    });
                } else if let Some(expected) = self.local_types.get(local).copied() {
                    self.verify_expression_type(expected, expression);
                }
            }
            HirExpressionKind::Parameter(parameter) => {
                let parameter_type = self.parameter_type(*parameter);
                if let Some(expected) = parameter_type {
                    self.verify_expression_type(expected, expression);
                } else {
                    self.errors.push(HirVerificationError::UnknownParameter {
                        parameter: *parameter,
                        span: expression.span(),
                    });
                }
            }
            HirExpressionKind::Capture(capture) => {
                if let Some(expected) = self.capture_types.get(capture).copied() {
                    self.verify_expression_type(expected, expression);
                } else {
                    self.errors.push(HirVerificationError::UnknownCapture {
                        capture: *capture,
                        span: expression.span(),
                    });
                }
            }
            HirExpressionKind::Closure(closure) => {
                self.verify_closure(closure, expression, visible);
            }
            HirExpressionKind::Function(function) => {
                self.verify_function(*function, expression.span());
                self.verify_function_reference(*function, expression);
            }
            HirExpressionKind::Field { .. }
            | HirExpressionKind::Record { .. }
            | HirExpressionKind::ClassConstruct { .. }
            | HirExpressionKind::RecordUpdate { .. }
            | HirExpressionKind::UnionCase { .. } => {
                self.verify_schema_expression(expression, visible);
            }
            HirExpressionKind::ResultCase {
                result,
                case,
                arguments,
            } => self.verify_result_case(*result, *case, arguments, expression, visible),
            HirExpressionKind::IterationCase {
                iteration,
                case,
                arguments,
            } => self.verify_iteration_case(*iteration, *case, arguments, expression, visible),
            HirExpressionKind::ErrorCase {
                error,
                case,
                arguments,
            } => self.verify_error_case(*error, *case, arguments, expression, visible),
            HirExpressionKind::EnumCase {
                definition,
                case,
                discriminant,
            } => {
                if self.schema.is_none() {
                    if self.arena.get(expression.type_id())
                        != Some(&SemanticType::Enum {
                            definition: *definition,
                        })
                    {
                        self.errors.push(HirVerificationError::InvalidType {
                            type_id: expression.type_id(),
                            span: expression.span(),
                        });
                    }
                    return;
                }
                let Some((expected_type, cases)) =
                    self.schema.and_then(|schema| schema.enums.get(definition))
                else {
                    self.errors.push(HirVerificationError::UnknownEnumCase {
                        enumeration: *definition,
                        case: *case,
                        span: expression.span(),
                    });
                    return;
                };
                if cases.get(case) != Some(discriminant) || *expected_type != expression.type_id() {
                    self.errors.push(HirVerificationError::UnknownEnumCase {
                        enumeration: *definition,
                        case: *case,
                        span: expression.span(),
                    });
                }
            }
            HirExpressionKind::ArrayGet { array, index } => {
                self.verify_array_get(array, index, visible);
            }
            HirExpressionKind::TableGet { table, key } => {
                self.verify_expression(table, visible);
                self.verify_expression(key, visible);
                let Some(SemanticType::Table {
                    key: key_type,
                    value: value_type,
                }) = self.arena.get(table.type_id())
                else {
                    self.errors
                        .push(HirVerificationError::InvalidCollectionType {
                            type_id: table.type_id(),
                            span: table.span(),
                        });
                    return;
                };
                self.verify_expression_type(*key_type, key);
                let nil = self.arena.source_type("nil");
                let valid_result = matches!(
                    self.arena.get(expression.type_id()),
                    Some(SemanticType::Union(members))
                        if members.len() == 2
                            && members.contains(value_type)
                            && nil.is_some_and(|nil| members.contains(&nil))
                );
                if !valid_result {
                    self.errors
                        .push(HirVerificationError::InvalidCollectionType {
                            type_id: expression.type_id(),
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::TupleGet { tuple, index } => {
                self.verify_expression(tuple, visible);
                let Some(SemanticType::Tuple(elements)) = self.arena.get(tuple.type_id()) else {
                    self.errors.push(HirVerificationError::InvalidFixedPack {
                        span: expression.span(),
                    });
                    return;
                };
                let Some(element) = elements.get(*index as usize) else {
                    self.errors.push(HirVerificationError::InvalidFixedPack {
                        span: expression.span(),
                    });
                    return;
                };
                self.verify_expression_type(*element, expression);
            }
            HirExpressionKind::ArrayCreate {
                length,
                initial_value,
            } => {
                self.verify_expression(length, visible);
                self.verify_expression(initial_value, visible);
                if let Some(integer) = self.arena.source_type("Int") {
                    self.verify_expression_type(integer, length);
                }
                if let Some(SemanticType::Array(element)) = self.arena.get(expression.type_id()) {
                    self.verify_expression_type(*element, initial_value);
                } else {
                    self.errors
                        .push(HirVerificationError::InvalidCollectionType {
                            type_id: expression.type_id(),
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::ArrayLength { array } => {
                self.verify_expression(array, visible);
                if !matches!(
                    self.arena.get(array.type_id()),
                    Some(SemanticType::Array(_))
                ) {
                    self.errors
                        .push(HirVerificationError::InvalidCollectionType {
                            type_id: array.type_id(),
                            span: array.span(),
                        });
                }
                if let Some(integer) = self.arena.source_type("Int") {
                    self.verify_expression_type(integer, expression);
                }
            }
            HirExpressionKind::ArrayGetChecked { array, index } => {
                self.verify_array_get(array, index, visible);
                if let Some(SemanticType::Array(element)) = self.arena.get(array.type_id()) {
                    self.verify_expression_type(*element, expression);
                }
            }
            HirExpressionKind::ArrayFill { array, value } => {
                self.verify_expression(array, visible);
                self.verify_expression(value, visible);
                if let Some(SemanticType::Array(element)) = self.arena.get(array.type_id()) {
                    self.verify_expression_type(*element, value);
                } else {
                    self.errors
                        .push(HirVerificationError::InvalidCollectionType {
                            type_id: array.type_id(),
                            span: array.span(),
                        });
                }
                if let Some(nil) = self.arena.source_type("nil") {
                    self.verify_expression_type(nil, expression);
                }
            }
            HirExpressionKind::ListCreate { capacity } => {
                if let Some(capacity) = capacity {
                    self.verify_expression(capacity, visible);
                    if let Some(integer) = self.arena.source_type("Int") {
                        self.verify_expression_type(integer, capacity);
                    }
                }
                if self.list_element_type(expression.type_id()).is_none() {
                    self.errors
                        .push(HirVerificationError::InvalidCollectionType {
                            type_id: expression.type_id(),
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::ListLength { list } => {
                self.verify_expression(list, visible);
                if self.list_element_type(list.type_id()).is_none() {
                    self.errors
                        .push(HirVerificationError::InvalidCollectionType {
                            type_id: list.type_id(),
                            span: list.span(),
                        });
                }
                if let Some(integer) = self.arena.source_type("Int") {
                    self.verify_expression_type(integer, expression);
                }
            }
            HirExpressionKind::ListGet { list, index } => {
                if let Some(element) = self.verify_list_get(list, index, visible) {
                    let nil = self.arena.source_type("nil");
                    let valid_result = matches!(
                        self.arena.get(expression.type_id()),
                        Some(SemanticType::Union(members))
                            if members.len() == 2
                                && members.contains(&element)
                                && nil.is_some_and(|nil| members.contains(&nil))
                    );
                    if !valid_result {
                        self.errors
                            .push(HirVerificationError::InvalidCollectionType {
                                type_id: expression.type_id(),
                                span: expression.span(),
                            });
                    }
                }
            }
            HirExpressionKind::ListGetChecked { list, index } => {
                if let Some(element) = self.verify_list_get(list, index, visible) {
                    self.verify_expression_type(element, expression);
                }
            }
            HirExpressionKind::ListAdd { list, value } => {
                self.verify_expression(list, visible);
                self.verify_expression(value, visible);
                if let Some(element) = self.list_element_type(list.type_id()) {
                    self.verify_expression_type(element, value);
                } else {
                    self.errors
                        .push(HirVerificationError::InvalidCollectionType {
                            type_id: list.type_id(),
                            span: list.span(),
                        });
                }
                if let Some(nil) = self.arena.source_type("nil") {
                    self.verify_expression_type(nil, expression);
                }
            }
            HirExpressionKind::RangeCreate { first, last, step } => {
                self.verify_expression(first, visible);
                self.verify_expression(last, visible);
                self.verify_expression(step, visible);
                let protocol = pop_types::embedded_bootstrap_schema()
                    .ok()
                    .and_then(|schema| schema.iteration_protocol());
                let valid_result = matches!(
                    (protocol, self.arena.get(expression.type_id())),
                    (
                        Some(protocol),
                        Some(SemanticType::Builtin { definition, arguments })
                    ) if *definition == protocol.range()
                        && arguments.as_slice() == [first.type_id()]
                        && matches!(
                            self.arena.get(first.type_id()),
                            Some(SemanticType::Primitive(pop_types::PrimitiveType::Integer(_)))
                        )
                );
                if !valid_result {
                    self.errors
                        .push(HirVerificationError::InvalidCollectionType {
                            type_id: expression.type_id(),
                            span: expression.span(),
                        });
                }
                self.verify_expression_type(first.type_id(), last);
                self.verify_expression_type(first.type_id(), step);
            }
            HirExpressionKind::Array(elements) => {
                self.verify_array(expression, elements, visible);
            }
            HirExpressionKind::Table(entries) => {
                self.verify_table(expression, entries, visible);
            }
            HirExpressionKind::Tuple(elements) => {
                for element in elements {
                    self.verify_expression(element, visible);
                }
                self.verify_tuple(expression, elements);
            }
            HirExpressionKind::Unary { operator, operand } => {
                self.verify_expression(operand, visible);
                self.verify_unary_operator(expression, *operator, operand);
            }
            HirExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                self.verify_expression(left, visible);
                self.verify_expression(right, visible);
                self.verify_binary_operator(expression, *operator, left, right);
            }
            HirExpressionKind::OptionalDefault { optional, fallback } => {
                self.verify_expression(optional, visible);
                self.verify_expression(fallback, visible);
                if let Some(inner) = self.optional_inner_type(optional.type_id()) {
                    self.verify_expression_type(inner, expression);
                    self.verify_expression_type(inner, fallback);
                } else {
                    self.errors.push(HirVerificationError::InvalidType {
                        type_id: optional.type_id(),
                        span: expression.span(),
                    });
                }
            }
            HirExpressionKind::OptionalPropagate {
                optional,
                enclosing_result,
            } => {
                self.verify_expression(optional, visible);
                if let Some(inner) = self.optional_inner_type(optional.type_id()) {
                    self.verify_expression_type(inner, expression);
                } else {
                    self.errors.push(HirVerificationError::InvalidType {
                        type_id: optional.type_id(),
                        span: expression.span(),
                    });
                }
                if self.results.as_slice() != [*enclosing_result]
                    || self.optional_inner_type(*enclosing_result).is_none()
                {
                    self.errors.push(HirVerificationError::InvalidType {
                        type_id: *enclosing_result,
                        span: expression.span(),
                    });
                }
            }
            HirExpressionKind::ResultPropagate {
                result,
                result_definition,
                success_type,
                error_type,
                enclosing_result,
            } => {
                self.verify_expression(result, visible);
                let operand_valid = matches!(
                    self.arena.get(result.type_id()),
                    Some(SemanticType::Builtin { definition, arguments })
                        if definition == result_definition
                            && arguments.as_slice() == [*success_type, *error_type]
                );
                let enclosing_valid = matches!(
                    self.arena.get(*enclosing_result),
                    Some(SemanticType::Builtin { definition, arguments })
                        if definition == result_definition
                            && arguments.len() == 2
                            && arguments[1] == *error_type
                );
                if !operand_valid
                    || !enclosing_valid
                    || self.results.as_slice() != [*enclosing_result]
                    || expression.type_id() != *success_type
                    || self.cleanup_depth != 0
                {
                    self.errors
                        .push(HirVerificationError::InvalidResultPropagation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::OptionalNarrow { optional } => {
                self.verify_expression(optional, visible);
                if let Some(inner) = self.optional_inner_type(optional.type_id()) {
                    self.verify_expression_type(inner, expression);
                } else {
                    self.errors.push(HirVerificationError::InvalidType {
                        type_id: optional.type_id(),
                        span: expression.span(),
                    });
                }
            }
            HirExpressionKind::Conditional {
                condition,
                when_true,
                when_false,
            } => {
                self.verify_expression(condition, visible);
                self.verify_condition(condition);
                self.verify_expression(when_true, visible);
                self.verify_expression(when_false, visible);
                self.verify_expression_type(expression.type_id(), when_true);
                self.verify_expression_type(expression.type_id(), when_false);
            }
            HirExpressionKind::Await { task } => {
                self.verify_expression(task, visible);
                if !self.is_async || self.cleanup_depth != self.async_cleanup_depth {
                    self.errors.push(HirVerificationError::AwaitOutsideAsync {
                        span: expression.span(),
                    });
                }
                let task_definition = embedded_bootstrap_schema()
                    .ok()
                    .and_then(|schema| schema.type_by_source_name("Task").copied())
                    .map(pop_types::BootstrapTypeEntry::id);
                let valid = matches!(
                    (task_definition, self.arena.get(task.type_id())),
                    (
                        Some(expected),
                        Some(SemanticType::Builtin {
                            definition,
                            arguments,
                        })
                    ) if *definition == expected
                        && arguments.as_slice() == [expression.type_id()]
                );
                if !valid {
                    self.errors.push(HirVerificationError::InvalidAwaitTask {
                        type_id: task.type_id(),
                        span: expression.span(),
                    });
                }
            }
            HirExpressionKind::TaskCancellationSource => {
                if !self.is_builtin_type(expression.type_id(), "Task.CancelSource", &[]) {
                    self.errors
                        .push(HirVerificationError::InvalidTaskOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::TaskCancelToken { source } => {
                self.verify_expression(source, visible);
                if !self.is_builtin_type(source.type_id(), "Task.CancelSource", &[])
                    || !self.is_builtin_type(expression.type_id(), "CancelToken", &[])
                {
                    self.errors
                        .push(HirVerificationError::InvalidTaskOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::TaskCancel { source } => {
                self.verify_expression(source, visible);
                if !self.is_builtin_type(source.type_id(), "Task.CancelSource", &[])
                    || self.arena.get(expression.type_id())
                        != Some(&SemanticType::Primitive(pop_types::PrimitiveType::Nil))
                {
                    self.errors
                        .push(HirVerificationError::InvalidTaskOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::TaskStart { group, task } => {
                self.verify_expression(group, visible);
                self.verify_expression(task, visible);
                let valid_task = embedded_bootstrap_schema()
                    .ok()
                    .and_then(|schema| schema.type_by_source_name("Task").copied())
                    .is_some_and(|entry| {
                        matches!(
                            self.arena.get(task.type_id()),
                            Some(SemanticType::Builtin { definition, arguments })
                                if *definition == entry.id() && arguments.len() == 1
                        )
                    });
                if !self.is_builtin_type(group.type_id(), "Task.Group", &[])
                    || !valid_task
                    || expression.type_id() != task.type_id()
                {
                    self.errors
                        .push(HirVerificationError::InvalidTaskOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiHandleOpen { value } => {
                self.verify_expression(value, visible);
                if !is_managed_reference_type(self.arena, value.type_id())
                    || ffi_handle_payload(self.arena, expression.type_id()) != Some(value.type_id())
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiHandleOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiHandleGet { handle } => {
                self.verify_expression(handle, visible);
                if ffi_handle_payload(self.arena, handle.type_id()) != Some(expression.type_id())
                    || !is_managed_reference_type(self.arena, expression.type_id())
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiHandleOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiHandleClose { handle } => {
                self.verify_expression(handle, visible);
                if ffi_handle_payload(self.arena, handle.type_id())
                    .is_none_or(|payload| !is_managed_reference_type(self.arena, payload))
                    || self.arena.get(expression.type_id())
                        != Some(&SemanticType::Primitive(PrimitiveType::Nil))
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiHandleOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiBufferOpen {
                length,
                element,
                layout_record,
            } => {
                self.verify_expression(length, visible);
                self.verify_type(*element, expression.span());
                let valid_result = self
                    .builtin_type_arguments(expression.type_id(), "Result")
                    .is_some_and(|arguments| {
                        matches!(arguments, [buffer, error]
                        if ffi_buffer_payload(self.arena, *buffer) == Some(*element)
                            && self.is_builtin_type(
                                *error,
                                "Ffi.AllocationError",
                                &[],
                            ))
                    });
                if !self.is_builtin_type(length.type_id(), "Ffi.C.Size", &[])
                    || !is_ffi_buffer_element(self.arena, *element, layout_record.is_some())
                    || layout_record.is_some_and(|record| {
                        self.schema.is_some_and(|schema| {
                            schema.records.get(&record).is_none_or(|record| {
                                record.type_id != *element || !record.ffi_c_layout
                            })
                        })
                    })
                    || !valid_result
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiBufferOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiBufferLength { buffer } => {
                self.verify_expression(buffer, visible);
                if ffi_buffer_payload(self.arena, buffer.type_id()).is_none()
                    || !self.is_builtin_type(expression.type_id(), "Ffi.C.Size", &[])
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiBufferOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiBufferRead { buffer, index } => {
                self.verify_expression(buffer, visible);
                self.verify_expression(index, visible);
                if ffi_buffer_payload(self.arena, buffer.type_id()) != Some(expression.type_id())
                    || !self.is_builtin_type(index.type_id(), "Ffi.C.Size", &[])
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiBufferOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiBufferWrite {
                buffer,
                index,
                value,
            } => {
                self.verify_expression(buffer, visible);
                self.verify_expression(index, visible);
                self.verify_expression(value, visible);
                if ffi_buffer_payload(self.arena, buffer.type_id()) != Some(value.type_id())
                    || !self.is_builtin_type(index.type_id(), "Ffi.C.Size", &[])
                    || self.arena.get(expression.type_id())
                        != Some(&SemanticType::Primitive(PrimitiveType::Nil))
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiBufferOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiBufferClose { buffer } => {
                self.verify_expression(buffer, visible);
                if ffi_buffer_payload(self.arena, buffer.type_id()).is_none()
                    || self.arena.get(expression.type_id())
                        != Some(&SemanticType::Primitive(PrimitiveType::Nil))
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiBufferOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiBufferWithPointer {
                buffer,
                body,
                body_type,
                element,
                layout_record,
                region: _,
            } => {
                self.verify_expression(buffer, visible);
                let closure_expression = HirExpression {
                    kind: HirExpressionKind::Nil,
                    type_id: *body_type,
                    span: body.span,
                };
                self.verify_closure(body, &closure_expression, visible);
                let valid_parameters = matches!(body.parameters.as_slice(), [pointer, length]
                    if ffi_exact_pointer_payload(
                        self.arena,
                        pointer.type_id,
                        pop_types::FFI_OPTIONAL_POINTER_TYPE_ID,
                    ) == Some(*element)
                    && self.is_builtin_type(length.type_id, "Ffi.C.Size", &[]));
                if ffi_buffer_payload(self.arena, buffer.type_id()) != Some(*element)
                    || body.is_async
                    || !valid_parameters
                    || body.results.as_slice() != [expression.type_id()]
                    || !is_ffi_buffer_element(self.arena, *element, layout_record.is_some())
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiBufferOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiBytesWithPin {
                bytes,
                body,
                body_type,
                region: _,
            } => {
                self.verify_expression(bytes, visible);
                let closure_expression = HirExpression {
                    kind: HirExpressionKind::Nil,
                    type_id: *body_type,
                    span: body.span,
                };
                self.verify_closure(body, &closure_expression, visible);
                let byte = self.arena.source_type("Byte");
                let valid_parameters = matches!((byte, body.parameters.as_slice()), (Some(byte), [pointer, length])
                    if ffi_exact_pointer_payload(
                        self.arena,
                        pointer.type_id,
                        pop_types::FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID,
                    ) == Some(byte)
                    && self.is_builtin_type(length.type_id, "Ffi.C.Size", &[]));
                if !self.is_builtin_type(bytes.type_id(), "Bytes", &[])
                    || body.is_async
                    || !valid_parameters
                    || body.results.as_slice() != [expression.type_id()]
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiBytesBorrow {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiWithCallback {
                callback,
                callback_type,
                binding_contract,
                body,
                body_type,
                site: _,
                region: _,
            } => {
                let callback_expression = HirExpression {
                    kind: HirExpressionKind::Nil,
                    type_id: *callback_type,
                    span: callback.span,
                };
                self.verify_closure(callback, &callback_expression, visible);
                let body_expression = HirExpression {
                    kind: HirExpressionKind::Nil,
                    type_id: *body_type,
                    span: body.span,
                };
                self.verify_closure(body, &body_expression, visible);
                if callback.is_async
                    || body.is_async
                    || !binding_contract.has_valid_shape()
                    || binding_contract.lifetime() != pop_types::FfiCallbackLifetime::CallScoped
                    || body.results.as_slice() != [expression.type_id()]
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiCallbackOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiCallbackOpen {
                callback,
                callback_type,
                thread: _,
                site: _,
            } => {
                let callback_expression = HirExpression {
                    kind: HirExpressionKind::Nil,
                    type_id: *callback_type,
                    span: callback.span,
                };
                self.verify_closure(callback, &callback_expression, visible);
                if callback.is_async {
                    self.errors
                        .push(HirVerificationError::InvalidFfiCallbackOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiCallbackWithPair {
                callback,
                callback_type: _,
                binding_contract,
                body,
                body_type,
                region: _,
            } => {
                self.verify_expression(callback, visible);
                let body_expression = HirExpression {
                    kind: HirExpressionKind::Nil,
                    type_id: *body_type,
                    span: body.span,
                };
                self.verify_closure(body, &body_expression, visible);
                if body.is_async
                    || !binding_contract.has_valid_shape()
                    || binding_contract.lifetime() != pop_types::FfiCallbackLifetime::Registered
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiCallbackOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiCallbackClose {
                callback,
                callback_type: _,
            } => self.verify_expression(callback, visible),
            HirExpressionKind::FfiPointerNone {
                element,
                layout_record,
                read_only,
            } => {
                self.verify_type(*element, expression.span());
                let expected = if *read_only {
                    pop_types::FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID
                } else {
                    pop_types::FFI_OPTIONAL_POINTER_TYPE_ID
                };
                if ffi_exact_pointer_payload(self.arena, expression.type_id(), expected)
                    != Some(*element)
                    || !is_ffi_buffer_element(self.arena, *element, layout_record.is_some())
                    || layout_record.is_some_and(|record| {
                        self.schema.is_some_and(|schema| {
                            schema.records.get(&record).is_none_or(|record| {
                                record.type_id != *element || !record.ffi_c_layout
                            })
                        })
                    })
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiPointerOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiPointerToOptional { pointer } => {
                self.verify_expression(pointer, visible);
                let constructors = match self.arena.get(pointer.type_id()) {
                    Some(SemanticType::Builtin {
                        definition,
                        arguments,
                    }) if *definition == pop_types::FFI_POINTER_TYPE_ID && arguments.len() == 1 => {
                        Some((
                            pop_types::FFI_POINTER_TYPE_ID,
                            pop_types::FFI_OPTIONAL_POINTER_TYPE_ID,
                        ))
                    }
                    Some(SemanticType::Builtin {
                        definition,
                        arguments,
                    }) if *definition == pop_types::FFI_READ_ONLY_POINTER_TYPE_ID
                        && arguments.len() == 1 =>
                    {
                        Some((
                            pop_types::FFI_READ_ONLY_POINTER_TYPE_ID,
                            pop_types::FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID,
                        ))
                    }
                    _ => None,
                };
                let valid = constructors.is_some_and(|(source, result)| {
                    let element = ffi_exact_pointer_payload(self.arena, pointer.type_id(), source);
                    element.is_some()
                        && ffi_exact_pointer_payload(self.arena, expression.type_id(), result)
                            == element
                });
                if !valid {
                    self.errors
                        .push(HirVerificationError::InvalidFfiPointerOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiPointerReadOnly { pointer } => {
                self.verify_expression(pointer, visible);
                let element = ffi_exact_pointer_payload(
                    self.arena,
                    pointer.type_id(),
                    pop_types::FFI_POINTER_TYPE_ID,
                );
                if element.is_none()
                    || ffi_exact_pointer_payload(
                        self.arena,
                        expression.type_id(),
                        pop_types::FFI_READ_ONLY_POINTER_TYPE_ID,
                    ) != element
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiPointerOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiPointerIsPresent { pointer } => {
                self.verify_expression(pointer, visible);
                let valid_source = ffi_exact_pointer_payload(
                    self.arena,
                    pointer.type_id(),
                    pop_types::FFI_OPTIONAL_POINTER_TYPE_ID,
                )
                .or_else(|| {
                    ffi_exact_pointer_payload(
                        self.arena,
                        pointer.type_id(),
                        pop_types::FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID,
                    )
                })
                .is_some();
                if !valid_source
                    || self.arena.get(expression.type_id())
                        != Some(&SemanticType::Primitive(PrimitiveType::Boolean))
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiPointerOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiPointerRequire {
                pointer,
                result,
                success,
                failure,
            } => {
                self.verify_expression(pointer, visible);
                let expected_success = ffi_exact_pointer_payload(
                    self.arena,
                    pointer.type_id(),
                    pop_types::FFI_OPTIONAL_POINTER_TYPE_ID,
                )
                .map(|element| (pop_types::FFI_POINTER_TYPE_ID, element))
                .or_else(|| {
                    ffi_exact_pointer_payload(
                        self.arena,
                        pointer.type_id(),
                        pop_types::FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID,
                    )
                    .map(|element| (pop_types::FFI_READ_ONLY_POINTER_TYPE_ID, element))
                });
                let valid_result = self
                    .builtin_type_arguments(expression.type_id(), "Result")
                    .is_some_and(|arguments| {
                        matches!(arguments, [pointer_result, error]
                        if expected_success.is_some_and(|(definition, element)| {
                            ffi_exact_pointer_payload(
                                self.arena,
                                *pointer_result,
                                definition,
                            ) == Some(element)
                        }) && self.is_builtin_type(
                            *error,
                            "Ffi.NullPointerError",
                            &[],
                        ))
                    });
                if self
                    .builtin_type_arguments(expression.type_id(), "Result")
                    .and_then(|_| embedded_bootstrap_schema().ok())
                    .and_then(|schema| schema.type_by_source_name("Result").map(|entry| entry.id()))
                    .is_none_or(|definition| definition != *result)
                    || success.raw() != 0
                    || failure.raw() != 1
                    || !valid_result
                {
                    self.errors
                        .push(HirVerificationError::InvalidFfiPointerOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::FfiUnsafeLoad {
                pointer,
                element,
                layout_record,
            } => {
                self.verify_expression(pointer, visible);
                let valid = ffi_exact_pointer_payload(
                    self.arena,
                    pointer.type_id(),
                    pop_types::FFI_READ_ONLY_POINTER_TYPE_ID,
                ) == Some(*element)
                    && expression.type_id() == *element
                    && valid_ffi_element_metadata(
                        self.arena,
                        self.schema,
                        *element,
                        *layout_record,
                    );
                self.verify_ffi_unsafe(expression, valid);
            }
            HirExpressionKind::FfiUnsafeStore {
                pointer,
                value,
                element,
                layout_record,
            } => {
                self.verify_expression(pointer, visible);
                self.verify_expression(value, visible);
                let valid = ffi_exact_pointer_payload(
                    self.arena,
                    pointer.type_id(),
                    pop_types::FFI_POINTER_TYPE_ID,
                ) == Some(*element)
                    && value.type_id() == *element
                    && self.arena.get(expression.type_id())
                        == Some(&SemanticType::Primitive(PrimitiveType::Nil))
                    && valid_ffi_element_metadata(
                        self.arena,
                        self.schema,
                        *element,
                        *layout_record,
                    );
                self.verify_ffi_unsafe(expression, valid);
            }
            HirExpressionKind::FfiUnsafeAdvance {
                pointer,
                elements,
                element,
                layout_record,
                read_only,
            } => {
                self.verify_expression(pointer, visible);
                self.verify_expression(elements, visible);
                let constructor = if *read_only {
                    pop_types::FFI_READ_ONLY_POINTER_TYPE_ID
                } else {
                    pop_types::FFI_POINTER_TYPE_ID
                };
                let valid = ffi_exact_pointer_payload(self.arena, pointer.type_id(), constructor)
                    == Some(*element)
                    && ffi_exact_pointer_payload(self.arena, expression.type_id(), constructor)
                        == Some(*element)
                    && self.is_builtin_type(elements.type_id(), "Ffi.C.PointerDifference", &[])
                    && valid_ffi_element_metadata(
                        self.arena,
                        self.schema,
                        *element,
                        *layout_record,
                    );
                self.verify_ffi_unsafe(expression, valid);
            }
            HirExpressionKind::FfiUnsafeCopy {
                source,
                destination,
                count,
                element,
                layout_record,
            } => {
                self.verify_expression(source, visible);
                self.verify_expression(destination, visible);
                self.verify_expression(count, visible);
                let valid = ffi_exact_pointer_payload(
                    self.arena,
                    source.type_id(),
                    pop_types::FFI_READ_ONLY_POINTER_TYPE_ID,
                ) == Some(*element)
                    && ffi_exact_pointer_payload(
                        self.arena,
                        destination.type_id(),
                        pop_types::FFI_POINTER_TYPE_ID,
                    ) == Some(*element)
                    && self.is_builtin_type(count.type_id(), "Ffi.C.Size", &[])
                    && self.arena.get(expression.type_id())
                        == Some(&SemanticType::Primitive(PrimitiveType::Nil))
                    && valid_ffi_element_metadata(
                        self.arena,
                        self.schema,
                        *element,
                        *layout_record,
                    );
                self.verify_ffi_unsafe(expression, valid);
            }
            HirExpressionKind::FfiUnsafeAddress {
                pointer,
                element,
                layout_record,
            } => {
                self.verify_expression(pointer, visible);
                let valid = ffi_exact_pointer_payload(
                    self.arena,
                    pointer.type_id(),
                    pop_types::FFI_READ_ONLY_POINTER_TYPE_ID,
                ) == Some(*element)
                    && self.is_builtin_type(expression.type_id(), "Ffi.C.Size", &[])
                    && valid_ffi_element_metadata(
                        self.arena,
                        self.schema,
                        *element,
                        *layout_record,
                    );
                self.verify_ffi_unsafe(expression, valid);
            }
            HirExpressionKind::FfiUnsafePointerFromAddress {
                address,
                element,
                layout_record,
            } => {
                self.verify_expression(address, visible);
                let valid = self.is_builtin_type(address.type_id(), "Ffi.C.Size", &[])
                    && ffi_exact_pointer_payload(
                        self.arena,
                        expression.type_id(),
                        pop_types::FFI_OPTIONAL_POINTER_TYPE_ID,
                    ) == Some(*element)
                    && valid_ffi_element_metadata(
                        self.arena,
                        self.schema,
                        *element,
                        *layout_record,
                    );
                self.verify_ffi_unsafe(expression, valid);
            }
            HirExpressionKind::TaskGroup { cancel, body } => {
                self.verify_expression(cancel, visible);
                self.verify_expression(body, visible);
                let completion = match self.arena.get(body.type_id()) {
                    Some(SemanticType::Function {
                        is_async: true,
                        parameters,
                        results,
                        ..
                    }) if parameters.len() == 1
                        && self.is_builtin_type(parameters[0], "Task.Group", &[]) =>
                    {
                        match results.as_slice() {
                            [result] => Some(*result),
                            _ => self.arena.find(&SemanticType::Tuple(results.clone())),
                        }
                    }
                    _ => None,
                };
                if !self.is_builtin_type(cancel.type_id(), "CancelToken", &[])
                    || !completion.is_some_and(|completion| {
                        self.is_builtin_type(expression.type_id(), "Task", &[completion])
                    })
                {
                    self.errors
                        .push(HirVerificationError::InvalidTaskOperation {
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::Call {
                dispatch,
                is_async,
                type_arguments,
                arguments,
            } => {
                self.verify_call(
                    dispatch,
                    *is_async,
                    type_arguments,
                    arguments,
                    Some(expression.type_id()),
                    expression.span(),
                    visible,
                );
            }
            HirExpressionKind::InterfaceUpcast { value, interface } => {
                self.verify_expression(value, visible);
                self.verify_interface_upcast(*interface, value, expression);
            }
            HirExpressionKind::NumericConvert { value, conversion } => {
                self.verify_expression(value, visible);
                if !valid_numeric_conversion(
                    *conversion,
                    value.type_id(),
                    expression.type_id(),
                    self.arena,
                ) {
                    self.errors
                        .push(HirVerificationError::InvalidNumericConversion {
                            conversion: *conversion,
                            source: value.type_id(),
                            target: expression.type_id(),
                            span: expression.span(),
                        });
                }
            }
            HirExpressionKind::StringConcat { left, right } => {
                self.verify_expression(left, visible);
                self.verify_expression(right, visible);
                let string = self.arena.source_type("String");
                if string != Some(left.type_id())
                    || string != Some(right.type_id())
                    || string != Some(expression.type_id())
                {
                    self.errors.push(HirVerificationError::InvalidType {
                        type_id: expression.type_id(),
                        span: expression.span(),
                    });
                }
            }
            HirExpressionKind::StringFormat { kind, value } => {
                self.verify_expression(value, visible);
                let source_valid = match (kind, self.arena.get(value.type_id())) {
                    (
                        pop_types::StringFormatKind::Boolean,
                        Some(SemanticType::Primitive(PrimitiveType::Boolean)),
                    ) => true,
                    (
                        pop_types::StringFormatKind::Integer(expected),
                        Some(SemanticType::Primitive(PrimitiveType::Integer(found))),
                    ) => expected == found,
                    (
                        pop_types::StringFormatKind::Float(FloatKind::Float32),
                        Some(SemanticType::Primitive(PrimitiveType::Float32)),
                    )
                    | (
                        pop_types::StringFormatKind::Float(FloatKind::Float64),
                        Some(SemanticType::Primitive(PrimitiveType::Float64)),
                    ) => true,
                    _ => false,
                };
                if !source_valid || self.arena.source_type("String") != Some(expression.type_id()) {
                    self.errors.push(HirVerificationError::InvalidType {
                        type_id: expression.type_id(),
                        span: expression.span(),
                    });
                }
            }
            HirExpressionKind::Integer(_) | HirExpressionKind::Float(_) => {
                self.verify_numeric_literal(expression);
            }
            HirExpressionKind::String(_)
            | HirExpressionKind::Boolean(_)
            | HirExpressionKind::Nil => self.verify_primitive_literal(expression),
        }
    }

    fn verify_ffi_unsafe(&mut self, expression: &HirExpression, valid: bool) {
        if !valid {
            self.errors
                .push(HirVerificationError::InvalidFfiUnsafeOperation {
                    span: expression.span(),
                });
        }
    }

    fn parameter_type(&self, parameter: ValueParameterId) -> Option<TypeId> {
        usize::try_from(parameter.raw())
            .ok()
            .and_then(|raw| self.parameter_types.get(raw))
            .copied()
    }

    fn optional_inner_type(&self, optional: TypeId) -> Option<TypeId> {
        let nil = self.arena.source_type("nil")?;
        let SemanticType::Union(members) = self.arena.get(optional)? else {
            return None;
        };
        if !members.contains(&nil) {
            return None;
        }
        let present = members
            .iter()
            .copied()
            .filter(|member| *member != nil)
            .collect::<Vec<_>>();
        match present.as_slice() {
            [inner] => Some(*inner),
            [] => None,
            _ => self.arena.find(&SemanticType::Union(present)),
        }
    }

    fn verify_optional_binding(
        &mut self,
        binding: BindingId,
        local: LocalId,
        inner_type: TypeId,
        initializer: &HirExpression,
        span: SourceSpan,
    ) {
        self.verify_type(inner_type, span);
        if self.optional_inner_type(initializer.type_id()) != Some(inner_type) {
            self.errors.push(HirVerificationError::InvalidType {
                type_id: initializer.type_id(),
                span,
            });
        }
        if self.local_types.insert(local, inner_type).is_some() {
            self.errors
                .push(HirVerificationError::DuplicateLocal(local));
        }
        self.local_bindings.insert(local, binding);
        if !self.bindings.insert(binding) {
            self.errors
                .push(HirVerificationError::DuplicateBinding(binding));
        }
    }

    fn parameter_binding(&self, parameter: ValueParameterId) -> Option<BindingId> {
        usize::try_from(parameter.raw())
            .ok()
            .and_then(|raw| self.parameter_bindings.get(raw))
            .copied()
    }

    #[allow(clippy::too_many_lines)]
    fn verify_closure(
        &mut self,
        closure: &HirClosure,
        expression: &HirExpression,
        visible: &BTreeSet<LocalId>,
    ) {
        if !self.nested_functions.insert(closure.function) {
            self.errors
                .push(HirVerificationError::DuplicateNestedFunction(
                    closure.function,
                ));
        }
        let expected_function = SemanticType::Function {
            is_async: closure.is_async,
            parameters: closure
                .parameters
                .iter()
                .map(HirClosureParameter::type_id)
                .collect(),
            results: closure.results.clone(),
            effects: pop_types::EffectSummary::empty(),
        };
        if self.arena.get(expression.type_id()) != Some(&expected_function) {
            self.errors.push(HirVerificationError::InvalidCallableType {
                type_id: expression.type_id(),
                span: expression.span(),
            });
        }

        let mut capture_ids = BTreeSet::new();
        let mut captured_bindings = BTreeSet::new();
        let mut previous_binding = None;
        let mut nested_capture_types = BTreeMap::new();
        let mut nested_capture_bindings = BTreeMap::new();
        let mut nested_capture_modes = BTreeMap::new();
        for capture in &closure.captures {
            self.verify_type(capture.type_id, closure.span);
            if !capture_ids.insert(capture.capture) {
                self.errors
                    .push(HirVerificationError::DuplicateCapture(capture.capture));
            }
            if !captured_bindings.insert(capture.binding) {
                self.errors
                    .push(HirVerificationError::DuplicateCapturedBinding(
                        capture.binding,
                    ));
            }
            if previous_binding.is_some_and(|previous| previous >= capture.binding) {
                self.errors
                    .push(HirVerificationError::InvalidCaptureSource {
                        capture: capture.capture,
                        binding: capture.binding,
                        span: closure.span,
                    });
            }
            previous_binding = Some(capture.binding);
            let source = match capture.source {
                HirCaptureSource::Local(local) if visible.contains(&local) => self
                    .local_types
                    .get(&local)
                    .copied()
                    .zip(self.local_bindings.get(&local).copied())
                    .map(|(type_id, binding)| (type_id, binding, None)),
                HirCaptureSource::Parameter(parameter) => self
                    .parameter_type(parameter)
                    .zip(self.parameter_binding(parameter))
                    .map(|(type_id, binding)| (type_id, binding, None)),
                HirCaptureSource::Capture(source) => self
                    .capture_types
                    .get(&source)
                    .copied()
                    .zip(self.capture_bindings.get(&source).copied())
                    .map(|(type_id, binding)| {
                        (type_id, binding, self.capture_modes.get(&source).copied())
                    }),
                HirCaptureSource::Local(_) => None,
            };
            let Some((source_type, source_binding, source_mode)) = source else {
                self.errors
                    .push(HirVerificationError::InvalidCaptureSource {
                        capture: capture.capture,
                        binding: capture.binding,
                        span: closure.span,
                    });
                continue;
            };
            if source_binding != capture.binding {
                self.errors
                    .push(HirVerificationError::InvalidCaptureSource {
                        capture: capture.capture,
                        binding: capture.binding,
                        span: closure.span,
                    });
            }
            if source_type != capture.type_id {
                self.errors.push(HirVerificationError::CaptureTypeMismatch {
                    capture: capture.capture,
                    expected: source_type,
                    found: capture.type_id,
                    span: closure.span,
                });
            }
            let expected_mode = if source_mode == Some(HirCaptureMode::Cell)
                || self.cell_bindings.contains(&capture.binding)
            {
                HirCaptureMode::Cell
            } else {
                HirCaptureMode::Value
            };
            if capture.mode != expected_mode {
                self.errors.push(HirVerificationError::CaptureModeMismatch {
                    capture: capture.capture,
                    span: closure.span,
                });
            }
            nested_capture_types.insert(capture.capture, capture.type_id);
            nested_capture_bindings.insert(capture.capture, capture.binding);
            nested_capture_modes.insert(capture.capture, capture.mode);
        }

        let saved_parameter_types = std::mem::replace(
            &mut self.parameter_types,
            closure
                .parameters
                .iter()
                .map(|parameter| parameter.type_id)
                .collect(),
        );
        let saved_parameter_bindings = std::mem::replace(
            &mut self.parameter_bindings,
            closure
                .parameters
                .iter()
                .map(|parameter| parameter.binding)
                .collect(),
        );
        let saved_results = std::mem::replace(&mut self.results, closure.results.clone());
        let saved_capture_types = std::mem::replace(&mut self.capture_types, nested_capture_types);
        let saved_capture_bindings =
            std::mem::replace(&mut self.capture_bindings, nested_capture_bindings);
        let saved_capture_modes = std::mem::replace(&mut self.capture_modes, nested_capture_modes);
        let nested_parameter_bindings = self.parameter_bindings.clone();
        let nested_cell_bindings = collect_cell_bindings(
            &closure.body,
            &nested_parameter_bindings,
            &self.capture_bindings,
            &self.capture_modes,
        );
        let saved_cell_bindings = std::mem::replace(&mut self.cell_bindings, nested_cell_bindings);
        let saved_loop_depth = std::mem::replace(&mut self.loop_depth, 0);
        let saved_cleanup_depth = std::mem::replace(&mut self.cleanup_depth, 0);
        let saved_async_cleanup_depth = std::mem::replace(&mut self.async_cleanup_depth, 0);
        let saved_is_async = std::mem::replace(&mut self.is_async, closure.is_async);
        for parameter in &closure.parameters {
            self.verify_type(parameter.type_id, parameter.span);
            if !self.bindings.insert(parameter.binding) {
                self.errors
                    .push(HirVerificationError::DuplicateBinding(parameter.binding));
            }
        }
        self.verify_statements(&closure.body, &BTreeSet::new());
        self.parameter_types = saved_parameter_types;
        self.parameter_bindings = saved_parameter_bindings;
        self.results = saved_results;
        self.capture_types = saved_capture_types;
        self.capture_bindings = saved_capture_bindings;
        self.capture_modes = saved_capture_modes;
        self.cell_bindings = saved_cell_bindings;
        self.loop_depth = saved_loop_depth;
        self.cleanup_depth = saved_cleanup_depth;
        self.async_cleanup_depth = saved_async_cleanup_depth;
        self.is_async = saved_is_async;
    }

    #[allow(clippy::too_many_lines)]
    fn verify_match(
        &mut self,
        scrutinee: &HirExpression,
        union: SymbolId,
        arms: &[HirMatchArm],
        span: SourceSpan,
        visible: &BTreeSet<LocalId>,
    ) {
        self.verify_expression(scrutinee, visible);
        let union_schema = self
            .schema
            .and_then(|schema| schema.unions.get(&union))
            .cloned();
        if self.schema.is_some() && union_schema.is_none() {
            self.errors
                .push(HirVerificationError::UnknownUnion { union, span });
        }
        if let Some(schema) = &union_schema
            && scrutinee.type_id() != schema.type_id
        {
            self.errors
                .push(HirVerificationError::MatchScrutineeTypeMismatch {
                    union,
                    expected: schema.type_id,
                    found: scrutinee.type_id(),
                    span: scrutinee.span(),
                });
        }
        let mut seen = BTreeSet::new();
        for arm in arms {
            if arm.union != union {
                self.errors.push(HirVerificationError::ForeignMatchCase {
                    expected_union: union,
                    found_union: arm.union,
                    case: arm.case,
                    span: arm.span,
                });
            }
            if !seen.insert(arm.case) {
                self.errors.push(HirVerificationError::DuplicateMatchCase {
                    union,
                    case: arm.case,
                    span: arm.span,
                });
            }
            let expected = union_schema
                .as_ref()
                .and_then(|schema| schema.cases.get(&arm.case));
            if union_schema.is_some() && expected.is_none() {
                self.errors.push(HirVerificationError::UnknownUnionCase {
                    union,
                    case: arm.case,
                    span: arm.span,
                });
            }
            if let Some(expected) = expected
                && expected.len() != arm.bindings.len()
            {
                self.errors
                    .push(HirVerificationError::MatchPayloadArityMismatch {
                        union,
                        case: arm.case,
                        expected: expected.len(),
                        found: arm.bindings.len(),
                        span: arm.span,
                    });
            }
            let mut arm_visible = visible.clone();
            for (index, binding) in arm.bindings.iter().enumerate() {
                self.verify_type(binding.type_id, binding.span);
                if let Some(expected) = expected.and_then(|types| types.get(index))
                    && *expected != binding.type_id
                {
                    self.errors
                        .push(HirVerificationError::MatchPayloadTypeMismatch {
                            union,
                            case: arm.case,
                            index,
                            expected: *expected,
                            found: binding.type_id,
                            span: binding.span,
                        });
                }
                match (binding.binding, binding.local, binding.name.as_str()) {
                    (None, None, "_") => {}
                    (Some(binding_id), Some(local), name) if name != "_" => {
                        if self.local_types.insert(local, binding.type_id).is_some() {
                            self.errors
                                .push(HirVerificationError::DuplicateLocal(local));
                        }
                        self.local_bindings.insert(local, binding_id);
                        if !self.bindings.insert(binding_id) {
                            self.errors
                                .push(HirVerificationError::DuplicateBinding(binding_id));
                        }
                        arm_visible.insert(local);
                    }
                    _ => self
                        .errors
                        .push(HirVerificationError::InvalidIgnoredMatchBinding {
                            span: binding.span,
                        }),
                }
            }
            self.verify_statements(&arm.body, &arm_visible);
        }
        if let Some(schema) = union_schema {
            for case in schema.cases.keys() {
                if !seen.contains(case) {
                    self.errors.push(HirVerificationError::MissingMatchCase {
                        union,
                        case: *case,
                        span,
                    });
                }
            }
        }
    }

    fn verify_error_match(
        &mut self,
        scrutinee: &HirExpression,
        error: ErrorId,
        arms: &[HirErrorMatchArm],
        span: SourceSpan,
        visible: &BTreeSet<LocalId>,
    ) {
        self.verify_expression(scrutinee, visible);
        let schema = self
            .schema
            .and_then(|schema| schema.errors.get(&error))
            .cloned();
        let valid_scrutinee = matches!(
            self.arena.get(scrutinee.type_id()),
            Some(SemanticType::ErrorUnion { definition, .. }) if *definition == error
        );
        if !valid_scrutinee || self.schema.is_some() && schema.is_none() {
            self.errors.push(HirVerificationError::InvalidErrorCase {
                error,
                case: ErrorCaseId::from_raw(u32::MAX),
                span,
            });
        }
        let mut seen = BTreeSet::new();
        for arm in arms {
            let expected = schema
                .as_ref()
                .and_then(|schema| schema.cases.get(&arm.case));
            if arm.error != error
                || !seen.insert(arm.case)
                || self.schema.is_some() && expected.is_none()
            {
                self.errors.push(HirVerificationError::InvalidErrorCase {
                    error,
                    case: arm.case,
                    span: arm.span,
                });
            }
            self.verify_match_bindings(
                &arm.bindings,
                expected.map(Vec::as_slice),
                &arm.body,
                visible,
            );
        }
        if let Some(schema) = schema {
            for case in schema.cases.keys() {
                if !seen.contains(case) {
                    self.errors.push(HirVerificationError::InvalidErrorCase {
                        error,
                        case: *case,
                        span,
                    });
                }
            }
        }
    }

    fn verify_result_match(
        &mut self,
        scrutinee: &HirExpression,
        result: pop_foundation::BuiltinTypeId,
        result_type: TypeId,
        arms: &[HirResultMatchArm],
        span: SourceSpan,
        visible: &BTreeSet<LocalId>,
    ) {
        self.verify_expression(scrutinee, visible);
        let parts = match self.arena.get(result_type) {
            Some(SemanticType::Builtin {
                definition,
                arguments,
            }) if *definition == result && arguments.len() == 2 => Some(arguments.clone()),
            _ => None,
        };
        if scrutinee.type_id() != result_type || parts.is_none() {
            self.errors.push(HirVerificationError::InvalidResultCase {
                case: ResultCaseId::from_raw(u32::MAX),
                span,
            });
        }
        let mut seen = BTreeSet::new();
        for arm in arms {
            let expected = usize::try_from(arm.case.raw())
                .ok()
                .and_then(|index| parts.as_ref().and_then(|parts| parts.get(index)))
                .map(std::slice::from_ref);
            if !seen.insert(arm.case) || expected.is_none() {
                self.errors.push(HirVerificationError::InvalidResultCase {
                    case: arm.case,
                    span: arm.span,
                });
            }
            self.verify_match_bindings(&arm.bindings, expected, &arm.body, visible);
        }
        for case in [ResultCaseId::from_raw(0), ResultCaseId::from_raw(1)] {
            if !seen.contains(&case) {
                self.errors
                    .push(HirVerificationError::InvalidResultCase { case, span });
            }
        }
    }

    fn verify_match_bindings(
        &mut self,
        bindings: &[HirMatchBinding],
        expected: Option<&[TypeId]>,
        body: &[HirStatement],
        visible: &BTreeSet<LocalId>,
    ) {
        let mut arm_visible = visible.clone();
        if expected.is_some_and(|types| types.len() != bindings.len()) {
            self.errors.push(HirVerificationError::InvalidFixedPack {
                span: bindings
                    .first()
                    .map_or_else(empty_span, HirMatchBinding::span),
            });
        }
        for (index, binding) in bindings.iter().enumerate() {
            self.verify_type(binding.type_id, binding.span);
            if expected
                .and_then(|types| types.get(index))
                .is_some_and(|expected| *expected != binding.type_id)
            {
                self.errors
                    .push(HirVerificationError::ExpressionTypeMismatch {
                        expected: expected
                            .and_then(|types| types.get(index))
                            .copied()
                            .unwrap(),
                        found: binding.type_id,
                        span: binding.span,
                    });
            }
            match (binding.binding, binding.local, binding.name.as_str()) {
                (None, None, "_") => {}
                (Some(binding_id), Some(local), name) if name != "_" => {
                    if self.local_types.insert(local, binding.type_id).is_some() {
                        self.errors
                            .push(HirVerificationError::DuplicateLocal(local));
                    }
                    self.local_bindings.insert(local, binding_id);
                    if !self.bindings.insert(binding_id) {
                        self.errors
                            .push(HirVerificationError::DuplicateBinding(binding_id));
                    }
                    arm_visible.insert(local);
                }
                _ => self
                    .errors
                    .push(HirVerificationError::InvalidIgnoredMatchBinding { span: binding.span }),
            }
        }
        self.verify_statements(body, &arm_visible);
    }

    fn verify_result_case(
        &mut self,
        result: pop_foundation::BuiltinTypeId,
        case: ResultCaseId,
        arguments: &[HirExpression],
        expression: &HirExpression,
        visible: &BTreeSet<LocalId>,
    ) {
        for argument in arguments {
            self.verify_expression(argument, visible);
        }
        let expected = match self.arena.get(expression.type_id()) {
            Some(SemanticType::Builtin {
                definition,
                arguments,
            }) if *definition == result => usize::try_from(case.raw())
                .ok()
                .and_then(|index| arguments.get(index))
                .copied(),
            _ => None,
        };
        if arguments.len() != 1 || expected != arguments.first().map(HirExpression::type_id) {
            self.errors.push(HirVerificationError::InvalidResultCase {
                case,
                span: expression.span(),
            });
        }
    }

    fn verify_iteration_case(
        &mut self,
        iteration: BuiltinTypeId,
        case: IterationCaseId,
        arguments: &[HirExpression],
        expression: &HirExpression,
        visible: &BTreeSet<LocalId>,
    ) {
        for argument in arguments {
            self.verify_expression(argument, visible);
        }
        let valid = match self.arena.get(expression.type_id()) {
            Some(SemanticType::Builtin {
                definition,
                arguments: type_arguments,
            }) if *definition == iteration && type_arguments.len() == 1 => {
                (case.raw() == 0
                    && arguments.len() == 1
                    && arguments[0].type_id() == type_arguments[0])
                    || (case.raw() == 1 && arguments.is_empty())
            }
            _ => false,
        };
        if !valid {
            self.errors
                .push(HirVerificationError::InvalidIterationCase {
                    case,
                    span: expression.span(),
                });
        }
    }

    fn verify_error_case(
        &mut self,
        error: ErrorId,
        case: ErrorCaseId,
        arguments: &[HirExpression],
        expression: &HirExpression,
        visible: &BTreeSet<LocalId>,
    ) {
        for argument in arguments {
            self.verify_expression(argument, visible);
        }
        let expected = self
            .schema
            .and_then(|schema| schema.errors.get(&error))
            .and_then(|schema| schema.cases.get(&case));
        let valid_type = matches!(
            self.arena.get(expression.type_id()),
            Some(SemanticType::ErrorUnion { definition, .. }) if *definition == error
        );
        let valid_arguments = expected.is_some_and(|types| {
            types.len() == arguments.len()
                && types
                    .iter()
                    .zip(arguments)
                    .all(|(expected, argument)| *expected == argument.type_id())
        });
        if !valid_type || self.schema.is_some() && !valid_arguments {
            self.errors.push(HirVerificationError::InvalidErrorCase {
                error,
                case,
                span: expression.span(),
            });
        }
    }

    fn verify_interface_upcast(
        &mut self,
        interface_id: NominalInterfaceId,
        value: &HirExpression,
        expression: &HirExpression,
    ) {
        let Some(schema) = self.schema else {
            return;
        };
        let source_class = schema
            .classes
            .values()
            .find(|class| class.type_id == value.type_id());
        let valid = match interface_id {
            NominalInterfaceId::User(interface_id) => {
                let Some(interface) = schema.interfaces.get(&interface_id) else {
                    self.errors.push(HirVerificationError::UnknownInterface {
                        interface: interface_id,
                        span: expression.span(),
                    });
                    return;
                };
                source_class.is_some_and(|class| class.interfaces.contains_key(&interface_id))
                    && expression.type_id() == interface.type_id
            }
            NominalInterfaceId::Builtin(interface_id) => source_class.is_some_and(|class| {
                class
                    .builtin_interfaces
                    .get(&interface_id)
                    .is_some_and(|implementation| {
                        implementation.interface_type == expression.type_id()
                    })
            }),
        };
        if !valid {
            self.errors
                .push(HirVerificationError::InvalidInterfaceUpcast {
                    interface: interface_id,
                    source: value.type_id(),
                    target: expression.type_id(),
                    span: expression.span(),
                });
        }
    }

    fn verify_schema_expression(
        &mut self,
        expression: &HirExpression,
        visible: &BTreeSet<LocalId>,
    ) {
        match expression.kind() {
            HirExpressionKind::Field { base, field } => {
                self.verify_expression(base, visible);
                self.verify_field_get(*field, base, expression);
            }
            HirExpressionKind::Record { record, fields } => {
                self.verify_fields(fields, visible);
                self.verify_record(*record, fields, true, expression);
            }
            HirExpressionKind::ClassConstruct {
                class,
                definition,
                fields,
            } => {
                self.verify_fields(fields, visible);
                self.verify_class(*class, *definition, fields, expression);
            }
            HirExpressionKind::RecordUpdate {
                record,
                base,
                fields,
            } => {
                self.verify_expression(base, visible);
                self.verify_fields(fields, visible);
                self.verify_record_update(*record, base, fields, expression);
            }
            HirExpressionKind::UnionCase {
                union,
                case,
                arguments,
            } => {
                for argument in arguments {
                    self.verify_expression(argument, visible);
                }
                self.verify_union_case(*union, *case, arguments, expression);
            }
            _ => unreachable!("schema expression verifier accepts only schema-owned expressions"),
        }
    }

    fn verify_return(&mut self, values: &[HirExpression], span: SourceSpan) {
        if values.len() != self.results.len() {
            self.errors.push(HirVerificationError::WrongReturnArity {
                expected: self.results.len(),
                found: values.len(),
                span,
            });
            return;
        }
        for (value, expected) in values.iter().zip(self.results.clone()) {
            self.verify_expression_type(expected, value);
        }
    }

    fn verify_condition(&mut self, condition: &HirExpression) {
        if self.arena.source_type("Boolean") != Some(condition.type_id()) {
            self.errors
                .push(HirVerificationError::InvalidConditionType {
                    found: condition.type_id(),
                    span: condition.span(),
                });
        }
    }

    fn verify_tuple(&mut self, expression: &HirExpression, elements: &[HirExpression]) {
        let Some(SemanticType::Tuple(element_types)) =
            self.arena.get(expression.type_id()).cloned()
        else {
            self.errors.push(HirVerificationError::InvalidType {
                type_id: expression.type_id(),
                span: expression.span(),
            });
            return;
        };
        if element_types.len() != elements.len() {
            self.errors.push(HirVerificationError::InvalidType {
                type_id: expression.type_id(),
                span: expression.span(),
            });
            return;
        }
        for (element, expected) in elements.iter().zip(element_types) {
            self.verify_expression_type(expected, element);
        }
    }

    fn verify_primitive_literal(&mut self, expression: &HirExpression) {
        let valid = matches!(
            (expression.kind(), self.arena.get(expression.type_id())),
            (
                HirExpressionKind::String(_),
                Some(SemanticType::Primitive(PrimitiveType::String))
            ) | (
                HirExpressionKind::Boolean(_),
                Some(SemanticType::Primitive(PrimitiveType::Boolean))
            ) | (
                HirExpressionKind::Nil,
                Some(SemanticType::Primitive(PrimitiveType::Nil))
            )
        );
        if !valid {
            self.errors.push(HirVerificationError::InvalidType {
                type_id: expression.type_id(),
                span: expression.span(),
            });
        }
    }

    fn verify_unary_operator(
        &mut self,
        expression: &HirExpression,
        operator: TypedUnaryOperator,
        operand: &HirExpression,
    ) {
        if !valid_hir_unary_operator(
            operator,
            operand.type_id(),
            expression.type_id(),
            self.arena,
        ) {
            self.errors
                .push(HirVerificationError::InvalidUnaryOperator {
                    operator,
                    operand: operand.type_id(),
                    result: expression.type_id(),
                    span: expression.span(),
                });
        }
    }

    fn verify_binary_operator(
        &mut self,
        expression: &HirExpression,
        operator: TypedBinaryOperator,
        left: &HirExpression,
        right: &HirExpression,
    ) {
        if !valid_hir_binary_operator(
            operator,
            left.type_id(),
            right.type_id(),
            expression.type_id(),
            self.arena,
        ) {
            self.errors
                .push(HirVerificationError::InvalidBinaryOperator {
                    operator,
                    left: left.type_id(),
                    right: right.type_id(),
                    result: expression.type_id(),
                    span: expression.span(),
                });
        }
    }

    fn verify_compound_operator(
        &mut self,
        operator: pop_types::TypedCompoundOperator,
        type_id: TypeId,
        span: SourceSpan,
    ) {
        let valid = match operator {
            pop_types::TypedCompoundOperator::Add
            | pop_types::TypedCompoundOperator::Subtract
            | pop_types::TypedCompoundOperator::Multiply
            | pop_types::TypedCompoundOperator::Divide => matches!(
                self.arena.get(type_id),
                Some(SemanticType::Primitive(
                    PrimitiveType::Integer(_) | PrimitiveType::Float32 | PrimitiveType::Float64
                ))
            ),
            pop_types::TypedCompoundOperator::Remainder => matches!(
                self.arena.get(type_id),
                Some(SemanticType::Primitive(PrimitiveType::Integer(_)))
            ),
            pop_types::TypedCompoundOperator::Concat => {
                self.arena.source_type("String") == Some(type_id)
            }
        };
        if !valid {
            self.errors
                .push(HirVerificationError::InvalidType { type_id, span });
        }
    }

    fn verify_numeric_literal(&mut self, expression: &HirExpression) {
        let matches = match expression.kind() {
            HirExpressionKind::Integer(value) => matches!(
                self.arena.get(expression.type_id()),
                Some(SemanticType::Primitive(PrimitiveType::Integer(kind)))
                    if *kind == value.kind()
            ),
            HirExpressionKind::Float(value) => matches!(
                (value.kind(), self.arena.get(expression.type_id())),
                (
                    pop_types::FloatKind::Float32,
                    Some(SemanticType::Primitive(PrimitiveType::Float32))
                ) | (
                    pop_types::FloatKind::Float64,
                    Some(SemanticType::Primitive(PrimitiveType::Float64))
                )
            ),
            _ => unreachable!("numeric literal verifier accepts only numeric literals"),
        };
        if !matches {
            self.errors.push(HirVerificationError::InvalidType {
                type_id: expression.type_id(),
                span: expression.span(),
            });
        }
    }

    fn verify_call(
        &mut self,
        dispatch: &HirCallDispatch,
        is_async: bool,
        type_arguments: &[TypeId],
        arguments: &[HirExpression],
        result: Option<TypeId>,
        span: SourceSpan,
        visible: &BTreeSet<LocalId>,
    ) {
        let signature = match dispatch {
            HirCallDispatch::Standard { function } => {
                if function.raw() == 0 {
                    self.arena
                        .source_type("Int")
                        .map(|int| HirCallableSignature {
                            is_async: false,
                            type_parameters: Vec::new(),
                            type_parameter_bounds: Vec::new(),
                            parameters: vec![int],
                            results: Vec::new(),
                        })
                } else {
                    None
                }
            }
            HirCallDispatch::Direct { function } => {
                self.verify_function(*function, span);
                self.schema
                    .and_then(|schema| schema.functions.get(function))
                    .cloned()
            }
            HirCallDispatch::Referenced { function } => {
                let signature = self
                    .schema
                    .and_then(|schema| schema.function_references.get(function))
                    .cloned();
                if self.schema.is_some() && signature.is_none() {
                    self.errors
                        .push(HirVerificationError::UnknownReferencedFunction {
                            function: *function,
                            span,
                        });
                }
                signature
            }
            HirCallDispatch::DirectMethod { method } => {
                if !self.known_methods.contains(method) {
                    self.errors.push(HirVerificationError::UnknownMethod {
                        method: *method,
                        span,
                    });
                }
                self.schema
                    .and_then(|schema| schema.methods.get(method))
                    .cloned()
            }
            HirCallDispatch::InterfaceMethod {
                interface,
                method,
                slot,
            } => {
                let signature =
                    self.schema
                        .and_then(|schema| schema.interfaces.get(interface))
                        .and_then(|interface_schema| {
                            interface_schema.methods.get(method).map(|method_schema| {
                                (interface_schema.type_id, method_schema.clone())
                            })
                        });
                if let Some((receiver_type, method_schema)) = signature {
                    if method_schema.slot != *slot {
                        self.errors
                            .push(HirVerificationError::WrongInterfaceMethodSlot {
                                interface: *interface,
                                method: *method,
                                expected: method_schema.slot,
                                found: *slot,
                                span,
                            });
                    }
                    let mut parameters = vec![receiver_type];
                    if let Some(receiver) = arguments.first()
                        && self.parameter_bounds.get(&receiver.type_id()) == Some(&receiver_type)
                    {
                        parameters[0] = receiver.type_id();
                    }
                    parameters.extend(method_schema.signature.parameters);
                    Some(HirCallableSignature {
                        is_async: false,
                        type_parameters: Vec::new(),
                        type_parameter_bounds: Vec::new(),
                        parameters,
                        results: method_schema.signature.results,
                    })
                } else {
                    if self.schema.is_some() {
                        self.errors
                            .push(HirVerificationError::UnknownInterfaceMethod {
                                interface: *interface,
                                method: *method,
                                span,
                            });
                    }
                    None
                }
            }
            HirCallDispatch::BuiltinInterfaceMethod { interface, method } => {
                let signature = arguments.first().and_then(|receiver| {
                    let receiver_contract = self
                        .parameter_bounds
                        .get(&receiver.type_id())
                        .copied()
                        .unwrap_or(receiver.type_id());
                    let SemanticType::Builtin {
                        definition,
                        arguments: interface_arguments,
                    } = self.arena.get(receiver_contract)?
                    else {
                        return None;
                    };
                    let [item_type] = interface_arguments.as_slice() else {
                        return None;
                    };
                    let protocol = embedded_bootstrap_schema()
                        .ok()
                        .and_then(|schema| schema.iteration_protocol())?;
                    if definition != interface {
                        return None;
                    }
                    let result_definition = if *method == protocol.iterator_method()
                        && (*interface == protocol.iterable() || *interface == protocol.iterator())
                    {
                        protocol.iterator()
                    } else if *method == protocol.next_method() && *interface == protocol.iterator()
                    {
                        protocol.iteration()
                    } else {
                        return None;
                    };
                    let result = self.arena.find(&SemanticType::Builtin {
                        definition: result_definition,
                        arguments: vec![*item_type],
                    })?;
                    Some(HirCallableSignature {
                        is_async: false,
                        type_parameters: Vec::new(),
                        type_parameter_bounds: Vec::new(),
                        parameters: vec![receiver.type_id()],
                        results: vec![result],
                    })
                });
                if signature.is_none() {
                    self.errors
                        .push(HirVerificationError::InvalidBuiltinInterfaceCall {
                            interface: *interface,
                            span,
                        });
                }
                signature
            }
            HirCallDispatch::Indirect { callee } => {
                self.verify_expression(callee, visible);
                if let Some(SemanticType::Function {
                    is_async,
                    parameters,
                    results,
                    ..
                }) = self.arena.get(callee.type_id()).cloned()
                {
                    Some(HirCallableSignature {
                        is_async,
                        type_parameters: Vec::new(),
                        type_parameter_bounds: Vec::new(),
                        parameters,
                        results,
                    })
                } else {
                    self.errors.push(HirVerificationError::InvalidCallableType {
                        type_id: callee.type_id(),
                        span: callee.span(),
                    });
                    None
                }
            }
        };
        for argument in arguments {
            self.verify_expression(argument, visible);
        }
        if self.schema.is_some()
            && let Some(mut signature) = signature
        {
            if signature.is_async != is_async {
                self.errors
                    .push(HirVerificationError::InvalidCallSignature {
                        expected_arguments: signature.parameters.len(),
                        found_arguments: arguments.len(),
                        expected_results: usize::from(signature.is_async),
                        found_results: usize::from(is_async),
                        span,
                    });
            }
            if signature.type_parameters.len() == type_arguments.len() && !type_arguments.is_empty()
            {
                let substitutions = signature
                    .type_parameters
                    .iter()
                    .zip(type_arguments)
                    .filter_map(|(parameter, argument)| match self.arena.get(*parameter) {
                        Some(SemanticType::TypeParameter(parameter)) => {
                            Some((*parameter, *argument))
                        }
                        _ => None,
                    })
                    .collect();
                signature.parameters = signature
                    .parameters
                    .iter()
                    .filter_map(|type_id| self.arena.substitute_existing(*type_id, &substitutions))
                    .collect();
                signature.results = signature
                    .results
                    .iter()
                    .filter_map(|type_id| self.arena.substitute_existing(*type_id, &substitutions))
                    .collect();
            }
            self.verify_call_signature(&signature, arguments, result, span);
        }
    }

    fn verify_call_signature(
        &mut self,
        signature: &HirCallableSignature,
        arguments: &[HirExpression],
        result: Option<TypeId>,
        span: SourceSpan,
    ) {
        for (index, (argument, expected)) in arguments.iter().zip(&signature.parameters).enumerate()
        {
            if argument.type_id() != *expected {
                self.errors
                    .push(HirVerificationError::CallArgumentTypeMismatch {
                        index,
                        expected: *expected,
                        found: argument.type_id(),
                        span: argument.span(),
                    });
            }
        }
        let expected_results = self.call_result_types(signature);
        let found_results = usize::from(result.is_some());
        if arguments.len() != signature.parameters.len() || expected_results.len() != found_results
        {
            self.errors
                .push(HirVerificationError::InvalidCallSignature {
                    expected_arguments: signature.parameters.len(),
                    found_arguments: arguments.len(),
                    expected_results: expected_results.len(),
                    found_results,
                    span,
                });
        }
        if let ([expected], Some(found)) = (expected_results.as_slice(), result)
            && *expected != found
        {
            self.errors
                .push(HirVerificationError::CallResultTypeMismatch {
                    expected: *expected,
                    found,
                    span,
                });
        }
    }

    fn call_result_types(&self, signature: &HirCallableSignature) -> Vec<TypeId> {
        if !signature.is_async {
            return signature.results.clone();
        }
        let completion = match signature.results.as_slice() {
            [completion] => Some(*completion),
            results => self.arena.find(&SemanticType::Tuple(results.to_vec())),
        };
        let task = embedded_bootstrap_schema()
            .ok()
            .and_then(|schema| schema.type_by_source_name("Task").copied())
            .map(pop_types::BootstrapTypeEntry::id);
        completion
            .zip(task)
            .and_then(|(completion, definition)| {
                self.arena.find(&SemanticType::Builtin {
                    definition,
                    arguments: vec![completion],
                })
            })
            .into_iter()
            .collect()
    }

    fn verify_function_reference(&mut self, function: SymbolId, expression: &HirExpression) {
        let Some(signature) = self
            .schema
            .and_then(|schema| schema.functions.get(&function))
            .cloned()
        else {
            return;
        };
        let expected = SemanticType::Function {
            is_async: signature.is_async,
            parameters: signature.parameters,
            results: signature.results,
            effects: pop_types::EffectSummary::empty(),
        };
        if self.arena.get(expression.type_id()) != Some(&expected) {
            self.errors
                .push(HirVerificationError::InvalidFunctionReferenceType {
                    function,
                    found: expression.type_id(),
                    span: expression.span(),
                });
        }
    }

    fn verify_record(
        &mut self,
        record: SymbolId,
        fields: &[HirFieldValue],
        require_complete: bool,
        expression: &HirExpression,
    ) {
        let Some(record_schema) = self
            .schema
            .and_then(|schema| schema.records.get(&record))
            .cloned()
        else {
            if self.schema.is_some() {
                self.errors.push(HirVerificationError::UnknownRecord {
                    record,
                    span: expression.span(),
                });
            }
            return;
        };
        self.verify_expression_type(record_schema.type_id, expression);
        self.verify_declared_fields(
            &record_schema.fields,
            fields,
            require_complete,
            expression.span(),
        );
    }

    fn verify_record_update(
        &mut self,
        record: SymbolId,
        base: &HirExpression,
        fields: &[HirFieldValue],
        expression: &HirExpression,
    ) {
        let Some(record_schema) = self
            .schema
            .and_then(|schema| schema.records.get(&record))
            .cloned()
        else {
            if self.schema.is_some() {
                self.errors.push(HirVerificationError::UnknownRecord {
                    record,
                    span: expression.span(),
                });
            }
            return;
        };
        self.verify_expression_type(record_schema.type_id, base);
        self.verify_expression_type(record_schema.type_id, expression);
        self.verify_declared_fields(&record_schema.fields, fields, false, expression.span());
    }

    fn verify_class(
        &mut self,
        class: ClassId,
        definition: SymbolId,
        fields: &[HirFieldValue],
        expression: &HirExpression,
    ) {
        let Some(class_schema) = self
            .schema
            .and_then(|schema| schema.classes.get(&class))
            .cloned()
        else {
            if self.schema.is_some() {
                self.errors.push(HirVerificationError::UnknownClass {
                    class,
                    span: expression.span(),
                });
            }
            return;
        };
        if definition != class_schema.definition {
            self.errors
                .push(HirVerificationError::WrongClassDefinition {
                    class,
                    expected: class_schema.definition,
                    found: definition,
                    span: expression.span(),
                });
        }
        self.verify_expression_type(class_schema.type_id, expression);
        self.verify_declared_fields(&class_schema.fields, fields, true, expression.span());
    }

    fn verify_declared_fields(
        &mut self,
        declared: &BTreeMap<FieldId, TypeId>,
        fields: &[HirFieldValue],
        require_complete: bool,
        span: SourceSpan,
    ) {
        let mut seen = BTreeSet::new();
        for field in fields {
            seen.insert(field.field());
            let Some(expected) = declared.get(&field.field()).copied() else {
                self.errors.push(HirVerificationError::UnknownField {
                    field: field.field(),
                    span: field.span(),
                });
                continue;
            };
            self.verify_expression_type(expected, field.value());
        }
        if require_complete {
            for field in declared.keys() {
                if !seen.contains(field) {
                    self.errors
                        .push(HirVerificationError::MissingDeclaredField {
                            field: *field,
                            span,
                        });
                }
            }
        }
    }

    fn verify_field_get(
        &mut self,
        field: FieldId,
        base: &HirExpression,
        expression: &HirExpression,
    ) {
        let Some(declared) = self
            .schema
            .and_then(|schema| schema.fields.get(&field))
            .cloned()
        else {
            if self.schema.is_some() {
                self.errors.push(HirVerificationError::UnknownField {
                    field,
                    span: expression.span(),
                });
            }
            return;
        };
        self.verify_field_owner(field, base, &declared, expression.span());
        self.verify_expression_type(declared.field_type, expression);
    }

    fn verify_field_set(
        &mut self,
        field: FieldId,
        base: &HirExpression,
        value: &HirExpression,
        span: SourceSpan,
    ) {
        let Some(declared) = self
            .schema
            .and_then(|schema| schema.fields.get(&field))
            .cloned()
        else {
            if self.schema.is_some() {
                self.errors
                    .push(HirVerificationError::UnknownField { field, span });
            }
            return;
        };
        self.verify_field_owner(field, base, &declared, span);
        self.verify_expression_type(declared.field_type, value);
        if !declared.mutable {
            self.errors
                .push(HirVerificationError::ImmutableFieldSet { field, span });
        }
    }

    fn verify_field_owner(
        &mut self,
        field: FieldId,
        base: &HirExpression,
        declared: &HirDeclaredField,
        span: SourceSpan,
    ) {
        if !declared.owners.contains(&base.type_id()) {
            self.errors.push(HirVerificationError::WrongFieldOwner {
                field,
                found: base.type_id(),
                span,
            });
        }
    }

    fn verify_union_case(
        &mut self,
        union: SymbolId,
        case: UnionCaseId,
        arguments: &[HirExpression],
        expression: &HirExpression,
    ) {
        let Some(union_schema) = self
            .schema
            .and_then(|schema| schema.unions.get(&union))
            .cloned()
        else {
            if self.schema.is_some() {
                self.errors.push(HirVerificationError::UnknownUnion {
                    union,
                    span: expression.span(),
                });
            }
            return;
        };
        self.verify_expression_type(union_schema.type_id, expression);
        let Some(parameters) = union_schema.cases.get(&case) else {
            self.errors.push(HirVerificationError::UnknownUnionCase {
                union,
                case,
                span: expression.span(),
            });
            return;
        };
        if parameters.len() != arguments.len() {
            self.errors
                .push(HirVerificationError::InvalidCallSignature {
                    expected_arguments: parameters.len(),
                    found_arguments: arguments.len(),
                    expected_results: 1,
                    found_results: 1,
                    span: expression.span(),
                });
        }
        for (index, (argument, expected)) in arguments.iter().zip(parameters).enumerate() {
            if argument.type_id() != *expected {
                self.errors
                    .push(HirVerificationError::UnionCaseArgumentTypeMismatch {
                        union,
                        case,
                        index,
                        expected: *expected,
                        found: argument.type_id(),
                        span: argument.span(),
                    });
            }
        }
    }

    fn verify_array_get(
        &mut self,
        array: &HirExpression,
        index: &HirExpression,
        visible: &BTreeSet<LocalId>,
    ) {
        self.verify_expression(array, visible);
        self.verify_expression(index, visible);
        if !matches!(
            self.arena.get(array.type_id()),
            Some(SemanticType::Array(_))
        ) {
            self.errors
                .push(HirVerificationError::InvalidCollectionType {
                    type_id: array.type_id(),
                    span: array.span(),
                });
        }
        if let Some(integer) = self.arena.source_type("Int") {
            self.verify_expression_type(integer, index);
        }
    }

    fn list_element_type(&self, type_id: TypeId) -> Option<TypeId> {
        let schema = pop_types::embedded_bootstrap_schema().ok()?;
        let list = schema.iteration_protocol()?.list();
        match self.arena.get(type_id)? {
            SemanticType::Builtin {
                definition,
                arguments,
            } if *definition == list && arguments.len() == 1 => Some(arguments[0]),
            _ => None,
        }
    }

    fn verify_list_get(
        &mut self,
        list: &HirExpression,
        index: &HirExpression,
        visible: &BTreeSet<LocalId>,
    ) -> Option<TypeId> {
        self.verify_expression(list, visible);
        self.verify_expression(index, visible);
        let element = self.list_element_type(list.type_id());
        if element.is_none() {
            self.errors
                .push(HirVerificationError::InvalidCollectionType {
                    type_id: list.type_id(),
                    span: list.span(),
                });
        }
        if let Some(integer) = self.arena.source_type("Int") {
            self.verify_expression_type(integer, index);
        }
        element
    }

    fn verify_array(
        &mut self,
        expression: &HirExpression,
        elements: &[HirExpression],
        visible: &BTreeSet<LocalId>,
    ) {
        let element_type = if let Some(SemanticType::Array(element_type)) =
            self.arena.get(expression.type_id()).cloned()
        {
            Some(element_type)
        } else {
            self.errors
                .push(HirVerificationError::InvalidCollectionType {
                    type_id: expression.type_id(),
                    span: expression.span(),
                });
            None
        };
        for element in elements {
            self.verify_expression(element, visible);
            if let Some(element_type) = element_type {
                self.verify_expression_type(element_type, element);
            }
        }
    }

    fn verify_table(
        &mut self,
        expression: &HirExpression,
        entries: &[HirTableEntry],
        visible: &BTreeSet<LocalId>,
    ) {
        let types = if let Some(SemanticType::Table { key, value }) =
            self.arena.get(expression.type_id()).cloned()
        {
            Some((key, value))
        } else {
            self.errors
                .push(HirVerificationError::InvalidCollectionType {
                    type_id: expression.type_id(),
                    span: expression.span(),
                });
            None
        };
        for entry in entries {
            self.verify_expression(entry.key(), visible);
            self.verify_expression(entry.value(), visible);
            if let Some((key, value)) = types {
                self.verify_expression_type(key, entry.key());
                self.verify_expression_type(value, entry.value());
            }
        }
    }

    fn verify_type(&mut self, type_id: TypeId, span: SourceSpan) {
        if !self.arena.is_valid_hir_type(type_id) {
            self.errors
                .push(HirVerificationError::InvalidType { type_id, span });
        }
    }

    fn verify_expression_type(&mut self, expected: TypeId, expression: &HirExpression) {
        if expression.type_id() != expected {
            self.errors
                .push(HirVerificationError::ExpressionTypeMismatch {
                    expected,
                    found: expression.type_id(),
                    span: expression.span(),
                });
        }
    }

    fn verify_fields(&mut self, fields: &[HirFieldValue], visible: &BTreeSet<LocalId>) {
        let mut seen = BTreeSet::new();
        for field in fields {
            if !seen.insert(field.field()) {
                self.errors
                    .push(HirVerificationError::DuplicateField(field.field()));
            }
            self.verify_expression(field.value(), visible);
        }
    }

    fn verify_function(&mut self, function: SymbolId, span: SourceSpan) {
        if !self.known_functions.contains(&function) {
            self.errors
                .push(HirVerificationError::UnknownFunction { function, span });
        }
    }
}

fn valid_hir_unary_operator(
    operator: TypedUnaryOperator,
    operand: TypeId,
    result: TypeId,
    arena: &TypeArena,
) -> bool {
    match operator {
        TypedUnaryOperator::Not => {
            arena.source_type("Boolean") == Some(operand) && operand == result
        }
        TypedUnaryOperator::Negate => {
            operand == result
                && (matches!(
                    arena.get(operand),
                    Some(SemanticType::Primitive(PrimitiveType::Integer(kind)))
                        if kind.is_signed()
                ) || is_hir_float(arena, operand))
        }
    }
}

fn valid_hir_binary_operator(
    operator: TypedBinaryOperator,
    left: TypeId,
    right: TypeId,
    result: TypeId,
    arena: &TypeArena,
) -> bool {
    let boolean = arena.source_type("Boolean");
    match operator {
        TypedBinaryOperator::Or | TypedBinaryOperator::And => {
            boolean == Some(left) && left == right && left == result
        }
        TypedBinaryOperator::Equal | TypedBinaryOperator::NotEqual => {
            boolean == Some(result)
                && ((left == right && hir_supports_default_equality(arena, left))
                    || arena.source_type("nil").is_some_and(|nil| {
                        left == nil && optional_inner_type(arena, right).is_some()
                            || right == nil && optional_inner_type(arena, left).is_some()
                    }))
        }
        TypedBinaryOperator::LessThan
        | TypedBinaryOperator::LessThanOrEqual
        | TypedBinaryOperator::GreaterThan
        | TypedBinaryOperator::GreaterThanOrEqual => {
            left == right && boolean == Some(result) && is_hir_numeric(arena, left)
        }
        TypedBinaryOperator::Add
        | TypedBinaryOperator::Subtract
        | TypedBinaryOperator::Multiply
        | TypedBinaryOperator::Divide => {
            left == right && left == result && is_hir_numeric(arena, left)
        }
        TypedBinaryOperator::Remainder => {
            left == right && left == result && is_hir_integer(arena, left)
        }
    }
}

fn optional_inner_type(arena: &TypeArena, optional: TypeId) -> Option<TypeId> {
    let nil = arena.source_type("nil")?;
    let SemanticType::Union(members) = arena.get(optional)? else {
        return None;
    };
    if !members.contains(&nil) {
        return None;
    }
    let present = members
        .iter()
        .copied()
        .filter(|member| *member != nil)
        .collect::<Vec<_>>();
    match present.as_slice() {
        [inner] => Some(*inner),
        [] => None,
        _ => arena.find(&SemanticType::Union(present)),
    }
}

fn valid_numeric_conversion(
    conversion: NumericConversionKind,
    source: TypeId,
    target: TypeId,
    arena: &TypeArena,
) -> bool {
    match conversion {
        NumericConversionKind::IntegerToInteger {
            source: source_kind,
            target: target_kind,
        } => {
            integer_kind(arena, source) == Some(source_kind)
                && integer_kind(arena, target) == Some(target_kind)
        }
        NumericConversionKind::IntegerToFloat {
            source: source_kind,
            target: target_kind,
        } => {
            integer_kind(arena, source) == Some(source_kind)
                && float_kind(arena, target) == Some(target_kind)
        }
        NumericConversionKind::FloatToInteger {
            source: source_kind,
            target: target_kind,
        } => {
            float_kind(arena, source) == Some(source_kind)
                && integer_kind(arena, target) == Some(target_kind)
        }
        NumericConversionKind::FloatToFloat {
            source: source_kind,
            target: target_kind,
        } => {
            float_kind(arena, source) == Some(source_kind)
                && float_kind(arena, target) == Some(target_kind)
        }
    }
}

fn integer_kind(arena: &TypeArena, type_id: TypeId) -> Option<pop_types::IntegerKind> {
    match arena.get(type_id) {
        Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => Some(*kind),
        _ => None,
    }
}

fn float_kind(arena: &TypeArena, type_id: TypeId) -> Option<FloatKind> {
    match arena.get(type_id) {
        Some(SemanticType::Primitive(PrimitiveType::Float32)) => Some(FloatKind::Float32),
        Some(SemanticType::Primitive(PrimitiveType::Float64)) => Some(FloatKind::Float64),
        _ => None,
    }
}

fn is_hir_numeric(arena: &TypeArena, type_id: TypeId) -> bool {
    is_hir_integer(arena, type_id) || is_hir_float(arena, type_id)
}

fn is_hir_integer(arena: &TypeArena, type_id: TypeId) -> bool {
    matches!(
        arena.get(type_id),
        Some(SemanticType::Primitive(PrimitiveType::Integer(_)))
    )
}

fn is_hir_float(arena: &TypeArena, type_id: TypeId) -> bool {
    matches!(
        arena.get(type_id),
        Some(SemanticType::Primitive(
            PrimitiveType::Float32 | PrimitiveType::Float64
        ))
    )
}

fn hir_supports_default_equality(arena: &TypeArena, type_id: TypeId) -> bool {
    match arena.get(type_id) {
        Some(
            SemanticType::Primitive(
                PrimitiveType::Nil
                | PrimitiveType::Boolean
                | PrimitiveType::Integer(_)
                | PrimitiveType::String,
            )
            | SemanticType::Class { .. }
            | SemanticType::Enum { .. },
        ) => true,
        Some(SemanticType::Tuple(elements) | SemanticType::Union(elements)) => elements
            .iter()
            .all(|element| hir_supports_default_equality(arena, *element)),
        Some(SemanticType::Record(fields)) => fields
            .iter()
            .all(|(_, field_type)| hir_supports_default_equality(arena, *field_type)),
        _ => false,
    }
}

fn collect_cell_bindings(
    statements: &[HirStatement],
    parameter_bindings: &[BindingId],
    capture_bindings: &BTreeMap<CaptureId, BindingId>,
    capture_modes: &BTreeMap<CaptureId, HirCaptureMode>,
) -> BTreeSet<BindingId> {
    let mut local_bindings = BTreeMap::new();
    collect_local_binding_map(statements, &mut local_bindings);
    let mut written = BTreeSet::new();
    for (capture, mode) in capture_modes {
        if *mode == HirCaptureMode::Cell
            && let Some(binding) = capture_bindings.get(capture)
        {
            written.insert(*binding);
        }
    }
    collect_written_bindings(
        statements,
        parameter_bindings,
        capture_bindings,
        &local_bindings,
        &mut written,
    );
    written
}

fn collect_local_binding_map(
    statements: &[HirStatement],
    local_bindings: &mut BTreeMap<LocalId, BindingId>,
) {
    for statement in statements {
        match statement.kind() {
            HirStatementKind::Local { local, binding, .. } => {
                local_bindings.insert(*local, *binding);
            }
            HirStatementKind::MultipleLocal { bindings, .. } => {
                for binding in bindings {
                    local_bindings.insert(binding.local, binding.binding);
                }
            }
            HirStatementKind::If {
                then_body,
                else_body,
                ..
            } => {
                collect_local_binding_map(then_body, local_bindings);
                collect_local_binding_map(else_body, local_bindings);
            }
            HirStatementKind::OptionalIf {
                local,
                binding,
                then_body,
                else_body,
                ..
            } => {
                local_bindings.insert(*local, *binding);
                collect_local_binding_map(then_body, local_bindings);
                collect_local_binding_map(else_body, local_bindings);
            }
            HirStatementKind::While { body, .. } | HirStatementKind::RepeatUntil { body, .. } => {
                collect_local_binding_map(body, local_bindings);
            }
            HirStatementKind::OptionalWhile {
                local,
                binding,
                body,
                ..
            } => {
                local_bindings.insert(*local, *binding);
                collect_local_binding_map(body, local_bindings);
            }
            HirStatementKind::NumericFor {
                local,
                binding,
                body,
                ..
            } => {
                local_bindings.insert(*local, *binding);
                collect_local_binding_map(body, local_bindings);
            }
            HirStatementKind::GeneralizedFor { bindings, body, .. } => {
                for binding in bindings {
                    local_bindings.insert(binding.local, binding.binding);
                }
                collect_local_binding_map(body, local_bindings);
            }
            HirStatementKind::Match { arms, .. } => {
                for arm in arms {
                    for binding in &arm.bindings {
                        if let (Some(binding), Some(local)) = (binding.binding, binding.local) {
                            local_bindings.insert(local, binding);
                        }
                    }
                    collect_local_binding_map(&arm.body, local_bindings);
                }
            }
            HirStatementKind::ErrorMatch { arms, .. } => {
                for arm in arms {
                    for binding in &arm.bindings {
                        if let (Some(binding), Some(local)) = (binding.binding, binding.local) {
                            local_bindings.insert(local, binding);
                        }
                    }
                    collect_local_binding_map(&arm.body, local_bindings);
                }
            }
            HirStatementKind::ResultMatch { arms, .. } => {
                for arm in arms {
                    for binding in &arm.bindings {
                        if let (Some(binding), Some(local)) = (binding.binding, binding.local) {
                            local_bindings.insert(local, binding);
                        }
                    }
                    collect_local_binding_map(&arm.body, local_bindings);
                }
            }
            HirStatementKind::Defer { body } | HirStatementKind::AsyncDefer { body } => {
                collect_local_binding_map(body, local_bindings);
            }
            HirStatementKind::LocalSet { .. }
            | HirStatementKind::ParameterSet { .. }
            | HirStatementKind::CaptureSet { .. }
            | HirStatementKind::Return { .. }
            | HirStatementKind::Break
            | HirStatementKind::Continue
            | HirStatementKind::FieldSet { .. }
            | HirStatementKind::CompoundFieldSet { .. }
            | HirStatementKind::ArraySet { .. }
            | HirStatementKind::ListSet { .. }
            | HirStatementKind::TableSet { .. }
            | HirStatementKind::CompoundArraySet { .. }
            | HirStatementKind::MultipleAssignment { .. }
            | HirStatementKind::Call(_)
            | HirStatementKind::Expression(_) => {}
        }
    }
}

fn collect_written_bindings(
    statements: &[HirStatement],
    parameter_bindings: &[BindingId],
    capture_bindings: &BTreeMap<CaptureId, BindingId>,
    local_bindings: &BTreeMap<LocalId, BindingId>,
    written: &mut BTreeSet<BindingId>,
) {
    for statement in statements {
        match statement.kind() {
            HirStatementKind::Local { initializer, .. } => {
                collect_cell_captures(initializer, written);
            }
            HirStatementKind::MultipleLocal { value, .. } => {
                collect_cell_captures(value, written);
            }
            HirStatementKind::LocalSet { local, value } => {
                if let Some(binding) = local_bindings.get(local) {
                    written.insert(*binding);
                }
                collect_cell_captures(value, written);
            }
            HirStatementKind::ParameterSet { parameter, value } => {
                if let Some(binding) = usize::try_from(parameter.raw())
                    .ok()
                    .and_then(|raw| parameter_bindings.get(raw))
                {
                    written.insert(*binding);
                }
                collect_cell_captures(value, written);
            }
            HirStatementKind::CaptureSet { capture, value } => {
                if let Some(binding) = capture_bindings.get(capture) {
                    written.insert(*binding);
                }
                collect_cell_captures(value, written);
            }
            HirStatementKind::Return { values } => {
                for value in values {
                    collect_cell_captures(value, written);
                }
            }
            HirStatementKind::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_cell_captures(condition, written);
                collect_written_bindings(
                    then_body,
                    parameter_bindings,
                    capture_bindings,
                    local_bindings,
                    written,
                );
                collect_written_bindings(
                    else_body,
                    parameter_bindings,
                    capture_bindings,
                    local_bindings,
                    written,
                );
            }
            HirStatementKind::OptionalIf {
                initializer,
                then_body,
                else_body,
                ..
            } => {
                collect_cell_captures(initializer, written);
                collect_written_bindings(
                    then_body,
                    parameter_bindings,
                    capture_bindings,
                    local_bindings,
                    written,
                );
                collect_written_bindings(
                    else_body,
                    parameter_bindings,
                    capture_bindings,
                    local_bindings,
                    written,
                );
            }
            HirStatementKind::While { condition, body } => {
                collect_cell_captures(condition, written);
                collect_written_bindings(
                    body,
                    parameter_bindings,
                    capture_bindings,
                    local_bindings,
                    written,
                );
            }
            HirStatementKind::OptionalWhile {
                initializer, body, ..
            } => {
                collect_cell_captures(initializer, written);
                collect_written_bindings(
                    body,
                    parameter_bindings,
                    capture_bindings,
                    local_bindings,
                    written,
                );
            }
            HirStatementKind::RepeatUntil { body, condition } => {
                collect_written_bindings(
                    body,
                    parameter_bindings,
                    capture_bindings,
                    local_bindings,
                    written,
                );
                collect_cell_captures(condition, written);
            }
            HirStatementKind::NumericFor {
                first,
                last,
                step,
                body,
                ..
            } => {
                collect_cell_captures(first, written);
                collect_cell_captures(last, written);
                collect_cell_captures(step, written);
                collect_written_bindings(
                    body,
                    parameter_bindings,
                    capture_bindings,
                    local_bindings,
                    written,
                );
            }
            HirStatementKind::GeneralizedFor { iterable, body, .. } => {
                collect_cell_captures(iterable, written);
                collect_written_bindings(
                    body,
                    parameter_bindings,
                    capture_bindings,
                    local_bindings,
                    written,
                );
            }
            HirStatementKind::Break | HirStatementKind::Continue => {}
            HirStatementKind::Match {
                scrutinee, arms, ..
            } => {
                collect_cell_captures(scrutinee, written);
                for arm in arms {
                    collect_written_bindings(
                        &arm.body,
                        parameter_bindings,
                        capture_bindings,
                        local_bindings,
                        written,
                    );
                }
            }
            HirStatementKind::ErrorMatch {
                scrutinee, arms, ..
            } => {
                collect_cell_captures(scrutinee, written);
                for arm in arms {
                    collect_written_bindings(
                        &arm.body,
                        parameter_bindings,
                        capture_bindings,
                        local_bindings,
                        written,
                    );
                }
            }
            HirStatementKind::ResultMatch {
                scrutinee, arms, ..
            } => {
                collect_cell_captures(scrutinee, written);
                for arm in arms {
                    collect_written_bindings(
                        &arm.body,
                        parameter_bindings,
                        capture_bindings,
                        local_bindings,
                        written,
                    );
                }
            }
            HirStatementKind::Defer { body } | HirStatementKind::AsyncDefer { body } => {
                collect_written_bindings(
                    body,
                    parameter_bindings,
                    capture_bindings,
                    local_bindings,
                    written,
                )
            }
            HirStatementKind::FieldSet { base, value, .. } => {
                collect_cell_captures(base, written);
                collect_cell_captures(value, written);
            }
            HirStatementKind::CompoundFieldSet { base, value, .. } => {
                collect_cell_captures(base, written);
                collect_cell_captures(value, written);
            }
            HirStatementKind::ArraySet {
                array,
                index,
                value,
            } => {
                collect_cell_captures(array, written);
                collect_cell_captures(index, written);
                collect_cell_captures(value, written);
            }
            HirStatementKind::ListSet { list, index, value } => {
                collect_cell_captures(list, written);
                collect_cell_captures(index, written);
                collect_cell_captures(value, written);
            }
            HirStatementKind::TableSet { table, key, value } => {
                collect_cell_captures(table, written);
                collect_cell_captures(key, written);
                collect_cell_captures(value, written);
            }
            HirStatementKind::CompoundArraySet {
                array,
                index,
                value,
                ..
            } => {
                collect_cell_captures(array, written);
                collect_cell_captures(index, written);
                collect_cell_captures(value, written);
            }
            HirStatementKind::MultipleAssignment { targets, value } => {
                for target in targets {
                    match target {
                        HirAssignmentTarget::Local { binding, .. }
                        | HirAssignmentTarget::Capture { binding, .. } => {
                            written.insert(*binding);
                        }
                        HirAssignmentTarget::Field { base, .. } => {
                            collect_cell_captures(base, written);
                        }
                        HirAssignmentTarget::Array { array, index, .. } => {
                            collect_cell_captures(array, written);
                            collect_cell_captures(index, written);
                        }
                        HirAssignmentTarget::List { list, index, .. } => {
                            collect_cell_captures(list, written);
                            collect_cell_captures(index, written);
                        }
                        HirAssignmentTarget::Table { table, key, .. } => {
                            collect_cell_captures(table, written);
                            collect_cell_captures(key, written);
                        }
                    }
                }
                collect_cell_captures(value, written);
            }
            HirStatementKind::Call(call) => {
                if let HirCallDispatch::Indirect { callee } = call.dispatch() {
                    collect_cell_captures(callee, written);
                }
                for argument in call.arguments() {
                    collect_cell_captures(argument, written);
                }
            }
            HirStatementKind::Expression(expression) => {
                collect_cell_captures(expression, written);
            }
        }
    }
}

fn collect_cell_captures(expression: &HirExpression, written: &mut BTreeSet<BindingId>) {
    match expression.kind() {
        HirExpressionKind::Closure(closure) => {
            for capture in &closure.captures {
                if capture.mode == HirCaptureMode::Cell {
                    written.insert(capture.binding);
                }
            }
        }
        HirExpressionKind::FfiBufferWithPointer { buffer, body, .. } => {
            collect_cell_captures(buffer, written);
            for capture in &body.captures {
                if capture.mode == HirCaptureMode::Cell {
                    written.insert(capture.binding);
                }
            }
        }
        HirExpressionKind::FfiBytesWithPin { bytes, body, .. } => {
            collect_cell_captures(bytes, written);
            for capture in &body.captures {
                if capture.mode == HirCaptureMode::Cell {
                    written.insert(capture.binding);
                }
            }
        }
        HirExpressionKind::FfiWithCallback { callback, body, .. } => {
            for closure in [callback, body] {
                for capture in &closure.captures {
                    if capture.mode == HirCaptureMode::Cell {
                        written.insert(capture.binding);
                    }
                }
            }
        }
        HirExpressionKind::FfiCallbackOpen { callback, .. } => {
            for capture in &callback.captures {
                if capture.mode == HirCaptureMode::Cell {
                    written.insert(capture.binding);
                }
            }
        }
        HirExpressionKind::FfiCallbackWithPair { callback, body, .. } => {
            collect_cell_captures(callback, written);
            for capture in &body.captures {
                if capture.mode == HirCaptureMode::Cell {
                    written.insert(capture.binding);
                }
            }
        }
        HirExpressionKind::FfiCallbackClose { callback, .. } => {
            collect_cell_captures(callback, written);
        }
        HirExpressionKind::Field { base, .. } => collect_cell_captures(base, written),
        HirExpressionKind::TupleGet { tuple, .. } => collect_cell_captures(tuple, written),
        HirExpressionKind::ArrayGet { array, index }
        | HirExpressionKind::ArrayGetChecked { array, index }
        | HirExpressionKind::ListGet { list: array, index }
        | HirExpressionKind::ListGetChecked { list: array, index } => {
            collect_cell_captures(array, written);
            collect_cell_captures(index, written);
        }
        HirExpressionKind::TableGet { table, key } => {
            collect_cell_captures(table, written);
            collect_cell_captures(key, written);
        }
        HirExpressionKind::ArrayCreate {
            length,
            initial_value,
        } => {
            collect_cell_captures(length, written);
            collect_cell_captures(initial_value, written);
        }
        HirExpressionKind::ArrayLength { array } => collect_cell_captures(array, written),
        HirExpressionKind::ListCreate { capacity } => {
            if let Some(capacity) = capacity {
                collect_cell_captures(capacity, written);
            }
        }
        HirExpressionKind::ListLength { list } => collect_cell_captures(list, written),
        HirExpressionKind::ListAdd { list, value } => {
            collect_cell_captures(list, written);
            collect_cell_captures(value, written);
        }
        HirExpressionKind::RangeCreate { first, last, step } => {
            collect_cell_captures(first, written);
            collect_cell_captures(last, written);
            collect_cell_captures(step, written);
        }
        HirExpressionKind::ArrayFill { array, value } => {
            collect_cell_captures(array, written);
            collect_cell_captures(value, written);
        }
        HirExpressionKind::Record { fields, .. }
        | HirExpressionKind::ClassConstruct { fields, .. } => {
            for field in fields {
                collect_cell_captures(field.value(), written);
            }
        }
        HirExpressionKind::RecordUpdate { base, fields, .. } => {
            collect_cell_captures(base, written);
            for field in fields {
                collect_cell_captures(field.value(), written);
            }
        }
        HirExpressionKind::Array(elements) | HirExpressionKind::Tuple(elements) => {
            for element in elements {
                collect_cell_captures(element, written);
            }
        }
        HirExpressionKind::Table(entries) => {
            for entry in entries {
                collect_cell_captures(entry.key(), written);
                collect_cell_captures(entry.value(), written);
            }
        }
        HirExpressionKind::UnionCase { arguments, .. }
        | HirExpressionKind::ResultCase { arguments, .. }
        | HirExpressionKind::IterationCase { arguments, .. }
        | HirExpressionKind::ErrorCase { arguments, .. }
        | HirExpressionKind::Call { arguments, .. } => {
            for argument in arguments {
                collect_cell_captures(argument, written);
            }
            if let HirExpressionKind::Call {
                dispatch: HirCallDispatch::Indirect { callee },
                ..
            } = expression.kind()
            {
                collect_cell_captures(callee, written);
            }
        }
        HirExpressionKind::Unary { operand, .. }
        | HirExpressionKind::Await { task: operand }
        | HirExpressionKind::TaskCancelToken { source: operand }
        | HirExpressionKind::TaskCancel { source: operand } => {
            collect_cell_captures(operand, written)
        }
        HirExpressionKind::FfiHandleOpen { value: operand }
        | HirExpressionKind::FfiHandleGet { handle: operand }
        | HirExpressionKind::FfiHandleClose { handle: operand }
        | HirExpressionKind::FfiBufferOpen {
            length: operand, ..
        }
        | HirExpressionKind::FfiBufferLength { buffer: operand }
        | HirExpressionKind::FfiBufferClose { buffer: operand } => {
            collect_cell_captures(operand, written)
        }
        HirExpressionKind::FfiPointerToOptional { pointer }
        | HirExpressionKind::FfiPointerReadOnly { pointer }
        | HirExpressionKind::FfiPointerIsPresent { pointer }
        | HirExpressionKind::FfiPointerRequire { pointer, .. } => {
            collect_cell_captures(pointer, written)
        }
        HirExpressionKind::FfiUnsafeLoad { pointer, .. }
        | HirExpressionKind::FfiUnsafeAddress { pointer, .. }
        | HirExpressionKind::FfiUnsafePointerFromAddress {
            address: pointer, ..
        } => collect_cell_captures(pointer, written),
        HirExpressionKind::FfiUnsafeStore { pointer, value, .. }
        | HirExpressionKind::FfiUnsafeAdvance {
            pointer,
            elements: value,
            ..
        } => {
            collect_cell_captures(pointer, written);
            collect_cell_captures(value, written);
        }
        HirExpressionKind::FfiUnsafeCopy {
            source,
            destination,
            count,
            ..
        } => {
            collect_cell_captures(source, written);
            collect_cell_captures(destination, written);
            collect_cell_captures(count, written);
        }
        HirExpressionKind::FfiBufferRead { buffer, index } => {
            collect_cell_captures(buffer, written);
            collect_cell_captures(index, written);
        }
        HirExpressionKind::FfiBufferWrite {
            buffer,
            index,
            value,
        } => {
            collect_cell_captures(buffer, written);
            collect_cell_captures(index, written);
            collect_cell_captures(value, written);
        }
        HirExpressionKind::TaskGroup { cancel, body } => {
            collect_cell_captures(cancel, written);
            collect_cell_captures(body, written);
        }
        HirExpressionKind::TaskStart { group, task } => {
            collect_cell_captures(group, written);
            collect_cell_captures(task, written);
        }
        HirExpressionKind::Binary { left, right, .. } => {
            collect_cell_captures(left, written);
            collect_cell_captures(right, written);
        }
        HirExpressionKind::OptionalDefault { optional, fallback } => {
            collect_cell_captures(optional, written);
            collect_cell_captures(fallback, written);
        }
        HirExpressionKind::OptionalPropagate { optional, .. }
        | HirExpressionKind::OptionalNarrow { optional } => {
            collect_cell_captures(optional, written);
        }
        HirExpressionKind::ResultPropagate { result, .. } => {
            collect_cell_captures(result, written);
        }
        HirExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            collect_cell_captures(condition, written);
            collect_cell_captures(when_true, written);
            collect_cell_captures(when_false, written);
        }
        HirExpressionKind::StringConcat { left, right } => {
            collect_cell_captures(left, written);
            collect_cell_captures(right, written);
        }
        HirExpressionKind::StringFormat { value, .. } => {
            collect_cell_captures(value, written);
        }
        HirExpressionKind::InterfaceUpcast { value, .. } => {
            collect_cell_captures(value, written);
        }
        HirExpressionKind::NumericConvert { value, .. } => {
            collect_cell_captures(value, written);
        }
        HirExpressionKind::Integer(_)
        | HirExpressionKind::Float(_)
        | HirExpressionKind::String(_)
        | HirExpressionKind::Boolean(_)
        | HirExpressionKind::Nil
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Parameter(_)
        | HirExpressionKind::Capture(_)
        | HirExpressionKind::Function(_)
        | HirExpressionKind::TaskCancellationSource
        | HirExpressionKind::FfiPointerNone { .. }
        | HirExpressionKind::EnumCase { .. } => {}
    }
}

fn empty_span() -> SourceSpan {
    SourceSpan::new(
        pop_foundation::FileId::from_raw(0),
        pop_foundation::TextRange::empty(pop_foundation::TextSize::from_u32(0)),
    )
}

fn method_span(method: &HirMethod) -> SourceSpan {
    method
        .parameters()
        .first()
        .map(HirParameter::span)
        .or_else(|| method.body().first().map(HirStatement::span))
        .unwrap_or_else(empty_span)
}
