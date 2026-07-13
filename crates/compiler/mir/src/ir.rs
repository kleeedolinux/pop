//! Canonical backend-neutral MIR implementation.
#![allow(
    clippy::match_same_arms,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

use std::fmt::Write;

use pop_foundation::{
    BindingId, BlockId, BubbleId, CaptureId, ClassId, FieldId, FunctionId, InterfaceId,
    InterfaceMethodId, MethodId, NamespaceId, NestedFunctionId, SourceSpan, StandardFunctionId,
    SymbolId, SymbolIdentity, TypeId, UnionCaseId, ValueId,
};
use pop_runtime_interface::{
    ArrayElementMap, ObjectMap, ObjectSlot, PanicPayload, SafePointId, StackMap, Trap, UnwindReason,
};
use pop_types::{FloatKind, FloatValue, IntegerKind, IntegerValue};

use crate::render::{
    dump_declaration, dump_function, dump_function_reference, dump_nested_function,
};
use crate::verification::instruction_operands;

pub(crate) const MAX_STRAIGHT_LINE_WORK_BETWEEN_SAFE_POINTS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MirEffect {
    Allocates,
    WritesManagedReference,
    MayTrap,
    MayUnwind,
    Suspends,
    UnsafeMemory,
    ForeignFunction,
    AmbientIo,
    CompilerQuery,
    GcSafePoint,
    Roots,
}

impl MirEffect {
    const fn bit(self) -> u16 {
        1_u16 << self as u16
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MirEffectSummary(u16);

impl MirEffectSummary {
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub fn from_effects(effects: impl IntoIterator<Item = MirEffect>) -> Self {
        effects.into_iter().fold(Self::empty(), Self::with)
    }

    #[must_use]
    pub const fn with(self, effect: MirEffect) -> Self {
        Self(self.0 | effect.bit())
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn is_subset_of(self, other: Self) -> bool {
        self.0 & !other.0 == 0
    }

    #[must_use]
    pub const fn contains(self, effect: MirEffect) -> bool {
        self.0 & effect.bit() != 0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = MirEffect> {
        const EFFECTS: [MirEffect; 11] = [
            MirEffect::Allocates,
            MirEffect::WritesManagedReference,
            MirEffect::MayTrap,
            MirEffect::MayUnwind,
            MirEffect::Suspends,
            MirEffect::UnsafeMemory,
            MirEffect::ForeignFunction,
            MirEffect::AmbientIo,
            MirEffect::CompilerQuery,
            MirEffect::GcSafePoint,
            MirEffect::Roots,
        ];
        EFFECTS
            .into_iter()
            .filter(move |effect| self.contains(*effect))
    }
}

pub(crate) fn lower_effect_summary(summary: pop_types::EffectSummary) -> MirEffectSummary {
    const EFFECTS: [(pop_types::Effect, MirEffect); 11] = [
        (pop_types::Effect::Allocates, MirEffect::Allocates),
        (
            pop_types::Effect::WritesManagedReference,
            MirEffect::WritesManagedReference,
        ),
        (pop_types::Effect::MayTrap, MirEffect::MayTrap),
        (pop_types::Effect::MayUnwind, MirEffect::MayUnwind),
        (pop_types::Effect::Suspends, MirEffect::Suspends),
        (pop_types::Effect::UnsafeMemory, MirEffect::UnsafeMemory),
        (
            pop_types::Effect::ForeignFunction,
            MirEffect::ForeignFunction,
        ),
        (pop_types::Effect::AmbientIo, MirEffect::AmbientIo),
        (pop_types::Effect::CompilerQuery, MirEffect::CompilerQuery),
        (pop_types::Effect::GcSafePoint, MirEffect::GcSafePoint),
        (pop_types::Effect::Roots, MirEffect::Roots),
    ];
    EFFECTS
        .into_iter()
        .filter_map(|(source, target)| summary.contains(source).then_some(target))
        .fold(MirEffectSummary::empty(), MirEffectSummary::with)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirUnwindAction {
    Propagate,
    Cleanup(BlockId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirBubble {
    pub(crate) bubble: BubbleId,
    pub(crate) namespace: NamespaceId,
    pub(crate) dependencies: Vec<BubbleId>,
    pub(crate) declarations: Vec<MirDeclaration>,
    pub(crate) functions: Vec<MirFunction>,
    pub(crate) methods: Vec<MirMethod>,
    pub(crate) nested_functions: Vec<MirNestedFunction>,
    pub(crate) function_references: Vec<MirFunctionReference>,
}

impl MirBubble {
    #[must_use]
    pub const fn bubble(&self) -> BubbleId {
        self.bubble
    }

    #[must_use]
    pub fn function_references(&self) -> &[MirFunctionReference] {
        &self.function_references
    }

    #[must_use]
    pub fn functions(&self) -> &[MirFunction] {
        &self.functions
    }

    #[must_use]
    pub fn declarations(&self) -> &[MirDeclaration] {
        &self.declarations
    }

    #[must_use]
    pub fn methods(&self) -> &[MirMethod] {
        &self.methods
    }

    #[must_use]
    pub fn nested_functions(&self) -> &[MirNestedFunction] {
        &self.nested_functions
    }

    #[must_use]
    pub fn dump(&self) -> String {
        let mut output = format!(
            "mir bubble b{} namespace n{}\n",
            self.bubble.raw(),
            self.namespace.raw()
        );
        output.push_str("dependencies");
        for dependency in &self.dependencies {
            let _ = write!(output, " b{}", dependency.raw());
        }
        output.push('\n');
        for reference in &self.function_references {
            dump_function_reference(&mut output, reference);
        }
        for declaration in &self.declarations {
            dump_declaration(&mut output, declaration);
        }
        for function in &self.functions {
            dump_function(&mut output, function);
        }
        for method in &self.methods {
            let _ = writeln!(
                output,
                "method m{} c{}",
                method.method.raw(),
                method.class.raw()
            );
            dump_function(&mut output, &method.function);
        }
        for function in &self.nested_functions {
            dump_nested_function(&mut output, function);
        }
        output
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirFunctionReference {
    pub(crate) identity: SymbolIdentity,
    pub(crate) parameters: Vec<TypeId>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) effects: MirEffectSummary,
}

impl MirFunctionReference {
    #[must_use]
    pub const fn identity(&self) -> SymbolIdentity {
        self.identity
    }

    #[must_use]
    pub fn parameters(&self) -> &[TypeId] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[TypeId] {
        &self.results
    }

    #[must_use]
    pub const fn effects(&self) -> MirEffectSummary {
        self.effects
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirDeclaration {
    pub(crate) symbol: SymbolId,
    pub(crate) kind: MirDeclarationKind,
}

impl MirDeclaration {
    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub const fn kind(&self) -> &MirDeclarationKind {
        &self.kind
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MirDeclarationKind {
    Record(MirRecordDeclaration),
    Union(MirUnionDeclaration),
    Class(MirClassDeclaration),
    Interface(MirInterfaceDeclaration),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirRecordDeclaration {
    pub(crate) type_id: TypeId,
    pub(crate) fields: Vec<MirField>,
}

impl MirRecordDeclaration {
    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn fields(&self) -> &[MirField] {
        &self.fields
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirUnionDeclaration {
    pub(crate) type_id: TypeId,
    pub(crate) cases: Vec<MirUnionCase>,
}

impl MirUnionDeclaration {
    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn cases(&self) -> &[MirUnionCase] {
        &self.cases
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirClassDeclaration {
    pub(crate) class: ClassId,
    pub(crate) type_id: TypeId,
    pub(crate) fields: Vec<MirField>,
    pub(crate) methods: Vec<MethodId>,
    pub(crate) interfaces: Vec<MirInterfaceImplementation>,
}

impl MirClassDeclaration {
    #[must_use]
    pub const fn class(&self) -> ClassId {
        self.class
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn fields(&self) -> &[MirField] {
        &self.fields
    }

    #[must_use]
    pub fn methods(&self) -> &[MethodId] {
        &self.methods
    }

    #[must_use]
    pub fn interfaces(&self) -> &[MirInterfaceImplementation] {
        &self.interfaces
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirInterfaceDeclaration {
    pub(crate) interface: InterfaceId,
    pub(crate) type_id: TypeId,
    pub(crate) methods: Vec<MirInterfaceMethod>,
}

impl MirInterfaceDeclaration {
    #[must_use]
    pub const fn interface(&self) -> InterfaceId {
        self.interface
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn methods(&self) -> &[MirInterfaceMethod] {
        &self.methods
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirInterfaceMethod {
    pub(crate) method: InterfaceMethodId,
    pub(crate) slot: u32,
    pub(crate) parameters: Vec<TypeId>,
    pub(crate) results: Vec<TypeId>,
}

impl MirInterfaceMethod {
    #[must_use]
    pub const fn method(&self) -> InterfaceMethodId {
        self.method
    }
    #[must_use]
    pub const fn slot(&self) -> u32 {
        self.slot
    }
    #[must_use]
    pub fn parameters(&self) -> &[TypeId] {
        &self.parameters
    }
    #[must_use]
    pub fn results(&self) -> &[TypeId] {
        &self.results
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirInterfaceImplementation {
    pub(crate) interface: InterfaceId,
    pub(crate) interface_type: TypeId,
    pub(crate) methods: Vec<MirInterfaceMethodImplementation>,
}

impl MirInterfaceImplementation {
    #[must_use]
    pub const fn interface(&self) -> InterfaceId {
        self.interface
    }
    #[must_use]
    pub const fn interface_type(&self) -> TypeId {
        self.interface_type
    }
    #[must_use]
    pub fn methods(&self) -> &[MirInterfaceMethodImplementation] {
        &self.methods
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirInterfaceMethodImplementation {
    pub(crate) interface_method: InterfaceMethodId,
    pub(crate) slot: u32,
    pub(crate) class_method: MethodId,
}

impl MirInterfaceMethodImplementation {
    #[must_use]
    pub const fn interface_method(self) -> InterfaceMethodId {
        self.interface_method
    }
    #[must_use]
    pub const fn slot(self) -> u32 {
        self.slot
    }
    #[must_use]
    pub const fn class_method(self) -> MethodId {
        self.class_method
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirField {
    pub(crate) field: FieldId,
    pub(crate) field_type: TypeId,
}

impl MirField {
    #[must_use]
    pub const fn field(&self) -> FieldId {
        self.field
    }

    #[must_use]
    pub const fn field_type(&self) -> TypeId {
        self.field_type
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirUnionCase {
    pub(crate) case: UnionCaseId,
    pub(crate) parameters: Vec<TypeId>,
}

impl MirUnionCase {
    #[must_use]
    pub const fn case(&self) -> UnionCaseId {
        self.case
    }

    #[must_use]
    pub fn parameters(&self) -> &[TypeId] {
        &self.parameters
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirMethod {
    pub(crate) method: MethodId,
    pub(crate) class: ClassId,
    pub(crate) function: MirFunction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirNestedFunction {
    pub(crate) owner: SymbolId,
    pub(crate) function: NestedFunctionId,
    pub(crate) captures: Vec<MirCapture>,
    pub(crate) parameters: Vec<TypeId>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) effects: MirEffectSummary,
    pub(crate) effects_explicit: bool,
    pub(crate) blocks: Vec<MirBlock>,
}

impl MirNestedFunction {
    #[must_use]
    pub const fn owner(&self) -> SymbolId {
        self.owner
    }
    #[must_use]
    pub const fn function(&self) -> NestedFunctionId {
        self.function
    }
    #[must_use]
    pub fn captures(&self) -> &[MirCapture] {
        &self.captures
    }
    #[must_use]
    pub fn parameters(&self) -> &[TypeId] {
        &self.parameters
    }
    #[must_use]
    pub fn results(&self) -> &[TypeId] {
        &self.results
    }
    #[must_use]
    pub const fn effects(&self) -> MirEffectSummary {
        self.effects
    }
    #[must_use]
    pub fn blocks(&self) -> &[MirBlock] {
        &self.blocks
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirCapture {
    pub(crate) capture: CaptureId,
    pub(crate) binding: BindingId,
    pub(crate) slot: u32,
    pub(crate) type_id: TypeId,
    pub(crate) mode: MirCaptureMode,
}

impl MirCapture {
    #[must_use]
    pub const fn capture(self) -> CaptureId {
        self.capture
    }
    #[must_use]
    pub const fn binding(self) -> BindingId {
        self.binding
    }
    #[must_use]
    pub const fn slot(self) -> u32 {
        self.slot
    }
    #[must_use]
    pub const fn type_id(self) -> TypeId {
        self.type_id
    }
    #[must_use]
    pub const fn mode(self) -> MirCaptureMode {
        self.mode
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirCaptureMode {
    Value,
    Cell,
}

impl MirMethod {
    #[must_use]
    pub const fn method(&self) -> MethodId {
        self.method
    }

    #[must_use]
    pub const fn class(&self) -> ClassId {
        self.class
    }

    #[must_use]
    pub const fn function(&self) -> &MirFunction {
        &self.function
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirFunction {
    pub(crate) function: FunctionId,
    pub(crate) symbol: SymbolId,
    pub(crate) parameters: Vec<TypeId>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) effects: MirEffectSummary,
    pub(crate) effects_explicit: bool,
    pub(crate) blocks: Vec<MirBlock>,
}

impl MirFunction {
    #[must_use]
    pub const fn function(&self) -> FunctionId {
        self.function
    }

    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub fn parameters(&self) -> &[TypeId] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[TypeId] {
        &self.results
    }

    #[must_use]
    pub const fn effects(&self) -> MirEffectSummary {
        self.effects
    }

    #[must_use]
    pub fn blocks(&self) -> &[MirBlock] {
        &self.blocks
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirBlock {
    pub(crate) block: BlockId,
    pub(crate) arguments: Vec<MirBlockArgument>,
    pub(crate) instructions: Vec<MirInstruction>,
    pub(crate) terminator: MirTerminator,
}

impl MirBlock {
    #[must_use]
    pub const fn block(&self) -> BlockId {
        self.block
    }

    #[must_use]
    pub fn arguments(&self) -> &[MirBlockArgument] {
        &self.arguments
    }

    #[must_use]
    pub fn instructions(&self) -> &[MirInstruction] {
        &self.instructions
    }

    #[must_use]
    pub const fn terminator(&self) -> &MirTerminator {
        &self.terminator
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirBlockArgument {
    pub(crate) value: ValueId,
    pub(crate) type_id: TypeId,
    pub(crate) span: SourceSpan,
}

impl MirBlockArgument {
    #[must_use]
    pub const fn value(self) -> ValueId {
        self.value
    }

    #[must_use]
    pub const fn type_id(self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirInstruction {
    pub(crate) result: ValueId,
    pub(crate) result_type: Option<TypeId>,
    pub(crate) kind: MirInstructionKind,
    pub(crate) effects: MirEffectSummary,
    pub(crate) effects_explicit: bool,
    pub(crate) span: SourceSpan,
}

impl MirInstruction {
    #[must_use]
    pub const fn result(&self) -> ValueId {
        self.result
    }

    #[must_use]
    /// Returns the SSA result type of a value-producing instruction.
    ///
    /// # Panics
    ///
    /// Panics when called for an explicit effect instruction with no result.
    /// Use [`Self::optional_result_type`] when either form is valid.
    pub const fn result_type(&self) -> TypeId {
        self.result_type
            .expect("value-producing MIR instruction has a result type")
    }

    #[must_use]
    pub const fn optional_result_type(&self) -> Option<TypeId> {
        self.result_type
    }

    #[must_use]
    pub const fn has_result(&self) -> bool {
        self.result_type.is_some()
    }

    #[must_use]
    pub const fn kind(&self) -> &MirInstructionKind {
        &self.kind
    }

    /// Returns the ordinary SSA operands read by this instruction.
    ///
    /// Precise GC roots are stack-map metadata and are intentionally not
    /// ordinary operands; consumers that transform roots must inspect the
    /// `GcSafePoint` instruction directly.
    #[must_use]
    pub fn operands(&self) -> Vec<ValueId> {
        instruction_operands(&self.kind)
    }

    #[must_use]
    pub const fn effects(&self) -> MirEffectSummary {
        self.effects
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MirInstructionKind {
    IntegerConstant(IntegerValue),
    FloatConstant(FloatValue),
    StringConstant(String),
    StringConcat {
        left: ValueId,
        right: ValueId,
    },
    StringFormat {
        kind: pop_types::StringFormatKind,
        value: ValueId,
    },
    BooleanConstant(bool),
    NilConstant,
    FunctionReference(SymbolId),
    TupleMake(Vec<ValueId>),
    TupleGet {
        tuple: ValueId,
        index: u32,
    },
    ArrayMake {
        elements: Vec<ValueId>,
        element_map: ArrayElementMap,
    },
    ArrayCreate {
        length: ValueId,
        initial_value: ValueId,
        element_map: ArrayElementMap,
    },
    TableMake {
        entries: Vec<(ValueId, ValueId)>,
        key_map: ArrayElementMap,
        value_map: ArrayElementMap,
    },
    ArrayGet {
        array: ValueId,
        index: ValueId,
    },
    ArrayLength {
        array: ValueId,
    },
    ArrayGetChecked {
        array: ValueId,
        index: ValueId,
    },
    ArraySet {
        array: ValueId,
        index: ValueId,
        value: ValueId,
        element_map: ArrayElementMap,
    },
    ArrayFill {
        array: ValueId,
        value: ValueId,
        element_map: ArrayElementMap,
    },
    CheckedIntegerAdd {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CheckedIntegerSubtract {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CheckedIntegerMultiply {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CheckedIntegerDivide {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CheckedIntegerRemainder {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    FloatAdd {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    FloatSubtract {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    FloatMultiply {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    FloatDivide {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    BooleanNot {
        operand: ValueId,
    },
    IntegerNegate {
        kind: IntegerKind,
        operand: ValueId,
    },
    FloatNegate {
        kind: FloatKind,
        operand: ValueId,
    },
    ConvertInteger {
        source: IntegerKind,
        target: IntegerKind,
        operand: ValueId,
    },
    ConvertIntegerToFloat {
        source: IntegerKind,
        target: FloatKind,
        operand: ValueId,
    },
    ConvertFloatToInteger {
        source: FloatKind,
        target: IntegerKind,
        operand: ValueId,
    },
    ConvertFloat {
        source: FloatKind,
        target: FloatKind,
        operand: ValueId,
    },
    BooleanAnd {
        left: ValueId,
        right: ValueId,
    },
    BooleanOr {
        left: ValueId,
        right: ValueId,
    },
    CompareEqual {
        left: ValueId,
        right: ValueId,
    },
    CompareNotEqual {
        left: ValueId,
        right: ValueId,
    },
    CompareIntegerLess {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CompareIntegerLessOrEqual {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CompareIntegerGreater {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CompareIntegerGreaterOrEqual {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CompareFloatLess {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    CompareFloatLessOrEqual {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    CompareFloatGreater {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    CompareFloatGreaterOrEqual {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    CallDirect {
        function: SymbolId,
        arguments: Vec<ValueId>,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    CallReferenced {
        function: SymbolIdentity,
        arguments: Vec<ValueId>,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    CallStandard {
        function: StandardFunctionId,
        arguments: Vec<ValueId>,
        declared_effects: MirEffectSummary,
    },
    CallDirectMethod {
        method: MethodId,
        arguments: Vec<ValueId>,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    CallInterface {
        interface: InterfaceId,
        method: InterfaceMethodId,
        slot: u32,
        arguments: Vec<ValueId>,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    CallIndirect {
        callee: ValueId,
        arguments: Vec<ValueId>,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    RecordMake {
        record: SymbolId,
        fields: Vec<(FieldId, ValueId)>,
    },
    ClassMake {
        class: ClassId,
        fields: Vec<(FieldId, ValueId)>,
        object_map: ObjectMap,
    },
    RecordUpdate {
        record: SymbolId,
        base: ValueId,
        fields: Vec<(FieldId, ValueId)>,
    },
    FieldGet {
        base: ValueId,
        field: FieldId,
    },
    FieldSet {
        base: ValueId,
        field: FieldId,
        value: ValueId,
    },
    UnionMake {
        union: SymbolId,
        case: UnionCaseId,
        arguments: Vec<ValueId>,
    },
    InterfaceUpcast {
        value: ValueId,
        interface: InterfaceId,
    },
    CaptureCellAllocate {
        binding: BindingId,
        initial: ValueId,
        value_type: TypeId,
        object_map: ObjectMap,
    },
    CaptureCellLoad {
        cell: ValueId,
    },
    CaptureCellStore {
        cell: ValueId,
        value: ValueId,
    },
    ClosureEnvironmentAllocate {
        owner: SymbolId,
        function: NestedFunctionId,
        captures: Vec<MirClosureCapture>,
        object_map: ObjectMap,
    },
    CaptureLoad {
        capture: CaptureId,
        slot: u32,
        mode: MirCaptureMode,
    },
    CaptureCellReference {
        capture: CaptureId,
        slot: u32,
    },
    CaptureStore {
        capture: CaptureId,
        slot: u32,
        value: ValueId,
    },
    GcSafePoint {
        safe_point: SafePointId,
        roots: Vec<ValueId>,
        stack_map: StackMap,
    },
    RetainRoot {
        value: ValueId,
    },
    ReleaseRoot {
        handle: ValueId,
    },
    Pin {
        value: ValueId,
    },
    Unpin {
        handle: ValueId,
    },
    WriteBarrier {
        owner: ValueId,
        slot: ObjectSlot,
        previous: Option<ValueId>,
        value: Option<ValueId>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirClosureCapture {
    pub(crate) capture: CaptureId,
    pub(crate) binding: BindingId,
    pub(crate) slot: u32,
    pub(crate) value: ValueId,
    pub(crate) self_reference: bool,
    pub(crate) type_id: TypeId,
    pub(crate) mode: MirCaptureMode,
}

impl MirClosureCapture {
    #[must_use]
    pub const fn capture(self) -> CaptureId {
        self.capture
    }
    #[must_use]
    pub const fn binding(self) -> BindingId {
        self.binding
    }
    #[must_use]
    pub const fn slot(self) -> u32 {
        self.slot
    }
    #[must_use]
    pub const fn value(self) -> ValueId {
        self.value
    }
    #[must_use]
    pub const fn self_reference(self) -> bool {
        self.self_reference
    }
    #[must_use]
    pub const fn type_id(self) -> TypeId {
        self.type_id
    }
    #[must_use]
    pub const fn mode(self) -> MirCaptureMode {
        self.mode
    }
}

impl MirInstructionKind {
    #[must_use]
    pub fn possible_traps(&self) -> Vec<pop_runtime_interface::TrapKind> {
        use pop_runtime_interface::TrapKind;
        match self {
            Self::CheckedIntegerAdd { .. }
            | Self::CheckedIntegerSubtract { .. }
            | Self::CheckedIntegerMultiply { .. }
            | Self::IntegerNegate { .. } => vec![TrapKind::IntegerOverflow],
            Self::CheckedIntegerDivide { .. } | Self::CheckedIntegerRemainder { .. } => {
                vec![TrapKind::IntegerOverflow, TrapKind::DivisionByZero]
            }
            Self::ConvertInteger { source, target, .. }
                if pop_types::NumericConversionKind::IntegerToInteger {
                    source: *source,
                    target: *target,
                }
                .may_trap() =>
            {
                vec![TrapKind::NumericConversion]
            }
            Self::ConvertFloatToInteger { .. } => vec![TrapKind::NumericConversion],
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MirTerminator {
    Missing,
    Branch {
        target: BlockId,
        arguments: Vec<ValueId>,
    },
    ConditionalBranch {
        condition: ValueId,
        when_true: BlockId,
        when_false: BlockId,
    },
    UnionSwitch {
        scrutinee: ValueId,
        union: SymbolId,
        arms: Vec<MirUnionSwitchArm>,
    },
    Return {
        values: Vec<ValueId>,
    },
    Trap(Trap),
    Panic(PanicPayload),
    ContinueUnwind(UnwindReason),
    Unreachable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirUnionSwitchArm {
    pub(crate) case: UnionCaseId,
    pub(crate) target: BlockId,
}

impl MirUnionSwitchArm {
    #[must_use]
    pub const fn case(self) -> UnionCaseId {
        self.case
    }
    #[must_use]
    pub const fn target(self) -> BlockId {
        self.target
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirVerificationError {
    InvalidType(TypeId),
    DuplicateValue(ValueId),
    UnknownValue(ValueId),
    ValueUsedBeforeDefinition(ValueId),
    ValueNotDominated {
        value: ValueId,
        definition: BlockId,
        use_block: BlockId,
    },
    InvalidBlock(BlockId),
    DuplicateBlock(BlockId),
    MissingTerminator(BlockId),
    EntryParameterArity {
        expected: usize,
        found: usize,
    },
    EntryParameterType {
        index: usize,
        expected: TypeId,
        found: TypeId,
    },
    EdgeArgumentArity {
        block: BlockId,
        target: BlockId,
        expected: usize,
        found: usize,
    },
    EdgeArgumentType {
        block: BlockId,
        target: BlockId,
        index: usize,
        expected: TypeId,
        found: TypeId,
    },
    ConditionalBranchConditionType {
        block: BlockId,
        found: TypeId,
    },
    WrongReturnArity {
        expected: usize,
        found: usize,
    },
    WrongReturnType {
        expected: TypeId,
        found: TypeId,
    },
    UnknownFunction(SymbolId),
    UnknownReferencedFunction(SymbolIdentity),
    UnknownMethod(MethodId),
    InvalidInstructionType {
        instruction: ValueId,
        result_type: TypeId,
    },
    WrongOperandType {
        instruction: ValueId,
        operand: ValueId,
        expected: TypeId,
        found: TypeId,
    },
    InvalidCollectionOperand {
        instruction: ValueId,
        operand: ValueId,
        found: TypeId,
    },
    InvalidCallableOperand {
        instruction: ValueId,
        operand: ValueId,
        found: TypeId,
    },
    InvalidCallSignature {
        instruction: ValueId,
        expected_arguments: usize,
        found_arguments: usize,
        expected_results: usize,
        found_results: usize,
    },
    InvalidComparisonOperands {
        instruction: ValueId,
        left: TypeId,
        right: TypeId,
    },
    DuplicateDeclaration(SymbolId),
    DuplicateReferencedFunction(SymbolIdentity),
    DuplicateClass(ClassId),
    DuplicateDeclaredField(FieldId),
    DuplicateUnionCase {
        union: SymbolId,
        case: UnionCaseId,
    },
    InvalidDeclarationType {
        symbol: SymbolId,
        type_id: TypeId,
    },
    UnknownRecord {
        instruction: ValueId,
        record: SymbolId,
    },
    UnknownClass {
        instruction: ValueId,
        class: ClassId,
    },
    UnknownUnion {
        instruction: ValueId,
        union: SymbolId,
    },
    UnknownField {
        instruction: ValueId,
        field: FieldId,
    },
    UnknownUnionCase {
        instruction: ValueId,
        union: SymbolId,
        case: UnionCaseId,
    },
    InvalidUnionSwitch {
        union: SymbolId,
    },
    UnknownInterface(InterfaceId),
    UnknownInterfaceMethod(InterfaceMethodId),
    UnknownStandardFunction(StandardFunctionId),
    WrongInterfaceMethodSlot {
        method: InterfaceMethodId,
        expected: u32,
        found: u32,
    },
    MissingDeclaredField {
        instruction: ValueId,
        field: FieldId,
    },
    WrongFieldOwner {
        instruction: ValueId,
        field: FieldId,
        expected: TypeId,
        found: TypeId,
    },
    ImmutableFieldSet {
        instruction: ValueId,
        field: FieldId,
    },
    FunctionEffectMismatch {
        function: SymbolId,
        expected: MirEffectSummary,
        found: MirEffectSummary,
    },
    InstructionEffectMismatch {
        instruction: ValueId,
        expected: MirEffectSummary,
        found: MirEffectSummary,
    },
    IncompleteStackMap {
        instruction: ValueId,
        expected: usize,
        found: usize,
    },
    InvalidStackMapRoot {
        instruction: ValueId,
        root: ValueId,
    },
    DuplicateRetain {
        instruction: ValueId,
        value: ValueId,
    },
    ReleaseWithoutRetain {
        instruction: ValueId,
        value: ValueId,
    },
    UnreleasedRoot {
        block: BlockId,
        value: ValueId,
    },
    RootStateMismatch(BlockId),
    InvalidPinnedReference {
        instruction: ValueId,
        value: ValueId,
    },
    UnpinWithoutPin {
        instruction: ValueId,
        value: ValueId,
    },
    UnreleasedPin {
        block: BlockId,
        value: ValueId,
    },
    PinStateMismatch(BlockId),
    InvalidObjectMap {
        instruction: ValueId,
    },
    MissingWriteBarrier {
        instruction: ValueId,
        field: FieldId,
    },
    UnexpectedWriteBarrier {
        instruction: ValueId,
    },
    InvalidUnwindAction {
        instruction: ValueId,
    },
    MissingGcSafePoint {
        instruction: ValueId,
    },
    MissingBackedgeSafePoint(BlockId),
}
