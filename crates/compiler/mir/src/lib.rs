//! Canonical backend-neutral control-flow IR and portable verification.
#![allow(
    clippy::match_same_arms,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use pop_foundation::{
    BindingId, BlockId, BubbleId, CaptureId, ClassId, FieldId, FileId, FunctionId, InterfaceId,
    InterfaceMethodId, LocalId, MethodId, NamespaceId, NestedFunctionId, SourceSpan,
    StandardFunctionId, SymbolId, TextRange, TextSize, TypeId, UnionCaseId, ValueId,
    ValueParameterId,
};
use pop_hir::{
    HirBubble, HirCallDispatch, HirCaptureMode, HirCaptureSource, HirClosure, HirDeclaration,
    HirDeclarationKind, HirExpression, HirExpressionKind, HirFieldValue, HirFunction, HirMatchArm,
    HirStatement, HirStatementKind, HirTableEntry,
};
use pop_runtime_interface::{
    ArrayElementMap, ObjectMap, ObjectSlot, PanicPayload, RootSlot, SafePointId, StackMap, Trap,
    UnwindReason,
};
use pop_types::{
    FloatKind, FloatValue, IntegerKind, IntegerValue, PrimitiveType, SemanticType, TypeArena,
    TypedBinaryOperator, TypedUnaryOperator,
};

mod optimize;
mod text;

pub use optimize::optimize_mir;
pub use text::{MirParseError, parse_mir_dump};

const MAX_STRAIGHT_LINE_WORK_BETWEEN_SAFE_POINTS: usize = 256;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirUnwindAction {
    Propagate,
    Cleanup(BlockId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirBubble {
    bubble: BubbleId,
    namespace: NamespaceId,
    dependencies: Vec<BubbleId>,
    declarations: Vec<MirDeclaration>,
    functions: Vec<MirFunction>,
    methods: Vec<MirMethod>,
    nested_functions: Vec<MirNestedFunction>,
}

impl MirBubble {
    #[must_use]
    pub const fn bubble(&self) -> BubbleId {
        self.bubble
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
pub struct MirDeclaration {
    symbol: SymbolId,
    kind: MirDeclarationKind,
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
    type_id: TypeId,
    fields: Vec<MirField>,
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
    type_id: TypeId,
    cases: Vec<MirUnionCase>,
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
    class: ClassId,
    type_id: TypeId,
    fields: Vec<MirField>,
    methods: Vec<MethodId>,
    interfaces: Vec<MirInterfaceImplementation>,
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
    interface: InterfaceId,
    type_id: TypeId,
    methods: Vec<MirInterfaceMethod>,
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
    method: InterfaceMethodId,
    slot: u32,
    parameters: Vec<TypeId>,
    results: Vec<TypeId>,
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
    interface: InterfaceId,
    interface_type: TypeId,
    methods: Vec<MirInterfaceMethodImplementation>,
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
    interface_method: InterfaceMethodId,
    slot: u32,
    class_method: MethodId,
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
    field: FieldId,
    field_type: TypeId,
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
    case: UnionCaseId,
    parameters: Vec<TypeId>,
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
    method: MethodId,
    class: ClassId,
    function: MirFunction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirNestedFunction {
    owner: SymbolId,
    function: NestedFunctionId,
    captures: Vec<MirCapture>,
    parameters: Vec<TypeId>,
    results: Vec<TypeId>,
    effects: MirEffectSummary,
    effects_explicit: bool,
    blocks: Vec<MirBlock>,
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
    capture: CaptureId,
    binding: BindingId,
    slot: u32,
    type_id: TypeId,
    mode: MirCaptureMode,
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
    function: FunctionId,
    symbol: SymbolId,
    parameters: Vec<TypeId>,
    results: Vec<TypeId>,
    effects: MirEffectSummary,
    effects_explicit: bool,
    blocks: Vec<MirBlock>,
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
    block: BlockId,
    arguments: Vec<MirBlockArgument>,
    instructions: Vec<MirInstruction>,
    terminator: MirTerminator,
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
    value: ValueId,
    type_id: TypeId,
    span: SourceSpan,
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
    result: ValueId,
    result_type: Option<TypeId>,
    kind: MirInstructionKind,
    effects: MirEffectSummary,
    effects_explicit: bool,
    span: SourceSpan,
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
    BooleanConstant(bool),
    NilConstant,
    FunctionReference(SymbolId),
    TupleMake(Vec<ValueId>),
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
    CompareIntegerGreater {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CompareFloatLess {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    CompareFloatGreater {
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
    capture: CaptureId,
    binding: BindingId,
    slot: u32,
    value: ValueId,
    self_reference: bool,
    type_id: TypeId,
    mode: MirCaptureMode,
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
    case: UnionCaseId,
    target: BlockId,
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

/// Lowers a verified HIR Bubble to canonical MIR and verifies the result.
///
/// # Errors
///
/// Returns deterministic MIR invariant violations.
pub fn lower_hir_bubble(
    hir: &HirBubble,
    arena: &TypeArena,
) -> Result<MirBubble, Vec<MirVerificationError>> {
    let declarations: Vec<_> = hir
        .declarations()
        .iter()
        .filter_map(lower_declaration)
        .collect();
    let gc_schema = LoweringGcSchema::new(&declarations, arena);
    let mut nested_functions = Vec::new();
    let mut functions: Vec<_> = hir
        .functions()
        .iter()
        .map(|function| {
            let (function, mut nested) = lower_function(function, arena, &gc_schema);
            nested_functions.append(&mut nested);
            function
        })
        .collect();
    functions.sort_by_key(MirFunction::symbol);
    let methods = hir
        .methods()
        .iter()
        .map(|method| {
            let (function, mut nested) = lower_function(method.function(), arena, &gc_schema);
            nested_functions.append(&mut nested);
            MirMethod {
                method: method.method(),
                class: method.class(),
                function,
            }
        })
        .collect();
    nested_functions.sort_by_key(|function| (function.owner(), function.function()));
    let mut mir = MirBubble {
        bubble: hir.bubble(),
        namespace: hir.namespace(),
        dependencies: hir.dependencies().to_vec(),
        declarations,
        functions,
        methods,
        nested_functions,
    };
    recompute_effects(&mut mir);
    while insert_gc_safe_points(&mut mir, arena) {
        // Backedge safe points make their containing function a GC safe point.
        // Recompute the transitive call effects before deciding which callers
        // also require a safe point immediately before the call.
        recompute_effects(&mut mir);
    }
    seal_effects(&mut mir);
    verify_mir_bubble(&mir, arena)?;
    Ok(mir)
}

fn seal_effects(bubble: &mut MirBubble) {
    for function in &mut bubble.functions {
        seal_function_effects(function);
    }
    for method in &mut bubble.methods {
        seal_function_effects(&mut method.function);
    }
    for function in &mut bubble.nested_functions {
        seal_nested_effects(function);
    }
}

fn seal_nested_effects(function: &mut MirNestedFunction) {
    function.effects_explicit = true;
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            instruction.effects_explicit = true;
        }
    }
}

fn seal_function_effects(function: &mut MirFunction) {
    function.effects_explicit = true;
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            instruction.effects_explicit = true;
        }
    }
}

fn lower_declaration(declaration: &HirDeclaration) -> Option<MirDeclaration> {
    let kind = match declaration.kind() {
        HirDeclarationKind::Record(record) => MirDeclarationKind::Record(MirRecordDeclaration {
            type_id: record.type_id(),
            fields: record
                .fields()
                .iter()
                .map(|field| MirField {
                    field: field.field(),
                    field_type: field.field_type(),
                })
                .collect(),
        }),
        HirDeclarationKind::Union(union) => MirDeclarationKind::Union(MirUnionDeclaration {
            type_id: union.type_id(),
            cases: union
                .cases()
                .iter()
                .map(|case| MirUnionCase {
                    case: case.case(),
                    parameters: case
                        .parameters()
                        .iter()
                        .map(pop_hir::HirNamedType::type_id)
                        .collect(),
                })
                .collect(),
        }),
        HirDeclarationKind::Class(class) => MirDeclarationKind::Class(MirClassDeclaration {
            class: class.class(),
            type_id: class.type_id(),
            fields: class
                .fields()
                .iter()
                .map(|field| MirField {
                    field: field.field(),
                    field_type: field.field_type(),
                })
                .collect(),
            methods: class
                .methods()
                .iter()
                .map(pop_hir::HirClassMethod::method)
                .collect(),
            interfaces: class
                .interfaces()
                .iter()
                .map(|implementation| MirInterfaceImplementation {
                    interface: implementation.interface(),
                    interface_type: implementation.interface_type(),
                    methods: implementation
                        .methods()
                        .iter()
                        .map(|method| MirInterfaceMethodImplementation {
                            interface_method: method.interface_method(),
                            slot: method.slot(),
                            class_method: method.class_method(),
                        })
                        .collect(),
                })
                .collect(),
        }),
        HirDeclarationKind::Interface(interface) => {
            MirDeclarationKind::Interface(MirInterfaceDeclaration {
                interface: interface.interface(),
                type_id: interface.type_id(),
                methods: interface
                    .methods()
                    .iter()
                    .map(|method| MirInterfaceMethod {
                        method: method.method(),
                        slot: method.slot(),
                        parameters: method
                            .parameters()
                            .iter()
                            .map(pop_hir::HirNamedType::type_id)
                            .collect(),
                        results: method.results().to_vec(),
                    })
                    .collect(),
            })
        }
        HirDeclarationKind::Attribute(_) => return None,
    };
    Some(MirDeclaration {
        symbol: declaration.symbol(),
        kind,
    })
}

fn lower_function(
    function: &HirFunction,
    arena: &TypeArena,
    gc_schema: &LoweringGcSchema,
) -> (MirFunction, Vec<MirNestedFunction>) {
    let (mut lowered, nested) = FunctionBuilder::new(function, arena, gc_schema).lower();
    lowered.function = function.function();
    (lowered, nested)
}

struct LoweringGcSchema {
    classes: BTreeMap<ClassId, ObjectMap>,
    fields: BTreeMap<FieldId, (ObjectSlot, TypeId)>,
}

impl LoweringGcSchema {
    fn new(declarations: &[MirDeclaration], arena: &TypeArena) -> Self {
        let mut classes = BTreeMap::new();
        let mut fields = BTreeMap::new();
        for declaration in declarations {
            let MirDeclarationKind::Class(class) = declaration.kind() else {
                continue;
            };
            let mut reference_slots = Vec::new();
            for (index, field) in class.fields().iter().enumerate() {
                let slot = ObjectSlot::new(u32::try_from(index).unwrap_or(u32::MAX));
                fields.insert(field.field(), (slot, field.field_type()));
                if is_managed_reference_type_id(field.field_type(), Some(arena)) {
                    reference_slots.push(slot);
                }
            }
            let slot_count = u32::try_from(class.fields().len()).unwrap_or(u32::MAX);
            let object_map = ObjectMap::new(slot_count, reference_slots)
                .expect("class field slots form a valid logical object map");
            classes.insert(class.class(), object_map);
        }
        Self { classes, fields }
    }
}

struct BuildingBlock {
    arguments: Vec<MirBlockArgument>,
    instructions: Vec<MirInstruction>,
    terminator: MirTerminator,
}

struct LiveState {
    parameters: Vec<ValueParameterId>,
    locals: Vec<LocalId>,
    specs: Vec<(TypeId, SourceSpan)>,
}

struct FunctionBuilder<'hir> {
    owner: SymbolId,
    parameters_schema: Vec<TypeId>,
    results: Vec<TypeId>,
    body: &'hir [HirStatement],
    capture_schema: BTreeMap<CaptureId, MirCapture>,
    arena: &'hir TypeArena,
    gc_schema: &'hir LoweringGcSchema,
    blocks: Vec<BuildingBlock>,
    current: BlockId,
    next_value: u32,
    parameters: BTreeMap<ValueParameterId, ValueId>,
    locals: BTreeMap<LocalId, ValueId>,
    parameter_cells: BTreeMap<ValueParameterId, ValueId>,
    local_cells: BTreeMap<LocalId, ValueId>,
    cell_parameters: BTreeSet<ValueParameterId>,
    cell_locals: BTreeSet<LocalId>,
    nested_functions: Vec<MirNestedFunction>,
}

fn collect_cell_sources(
    statements: &[HirStatement],
) -> (BTreeSet<ValueParameterId>, BTreeSet<LocalId>) {
    let mut parameters = BTreeSet::new();
    let mut locals = BTreeSet::new();
    for statement in statements {
        visit_statement_closures(statement, &mut parameters, &mut locals);
    }
    (parameters, locals)
}

fn visit_statement_closures(
    statement: &HirStatement,
    parameters: &mut BTreeSet<ValueParameterId>,
    locals: &mut BTreeSet<LocalId>,
) {
    match statement.kind() {
        HirStatementKind::Local { initializer, .. } => {
            visit_expression_closures(initializer, parameters, locals);
        }
        HirStatementKind::LocalSet { value, .. }
        | HirStatementKind::ParameterSet { value, .. }
        | HirStatementKind::CaptureSet { value, .. }
        | HirStatementKind::Expression(value) => {
            visit_expression_closures(value, parameters, locals);
        }
        HirStatementKind::Return { values } => {
            for value in values {
                visit_expression_closures(value, parameters, locals);
            }
        }
        HirStatementKind::If {
            condition,
            then_body,
            else_body,
        } => {
            visit_expression_closures(condition, parameters, locals);
            for nested in then_body.iter().chain(else_body) {
                visit_statement_closures(nested, parameters, locals);
            }
        }
        HirStatementKind::While { condition, body } => {
            visit_expression_closures(condition, parameters, locals);
            for nested in body {
                visit_statement_closures(nested, parameters, locals);
            }
        }
        HirStatementKind::RepeatUntil { body, condition } => {
            for nested in body {
                visit_statement_closures(nested, parameters, locals);
            }
            visit_expression_closures(condition, parameters, locals);
        }
        HirStatementKind::Match {
            scrutinee, arms, ..
        } => {
            visit_expression_closures(scrutinee, parameters, locals);
            for arm in arms {
                for nested in arm.body() {
                    visit_statement_closures(nested, parameters, locals);
                }
            }
        }
        HirStatementKind::FieldSet { base, value, .. } => {
            visit_expression_closures(base, parameters, locals);
            visit_expression_closures(value, parameters, locals);
        }
        HirStatementKind::ArraySet {
            array,
            index,
            value,
        } => {
            visit_expression_closures(array, parameters, locals);
            visit_expression_closures(index, parameters, locals);
            visit_expression_closures(value, parameters, locals);
        }
        HirStatementKind::Call(call) => {
            for argument in call.arguments() {
                visit_expression_closures(argument, parameters, locals);
            }
        }
    }
}

fn visit_expression_closures(
    expression: &HirExpression,
    parameters: &mut BTreeSet<ValueParameterId>,
    locals: &mut BTreeSet<LocalId>,
) {
    match expression.kind() {
        HirExpressionKind::Closure(closure) => {
            for capture in closure.captures() {
                if capture.mode() != HirCaptureMode::Cell {
                    continue;
                }
                match capture.source() {
                    HirCaptureSource::Local(local) => {
                        locals.insert(local);
                    }
                    HirCaptureSource::Parameter(parameter) => {
                        parameters.insert(parameter);
                    }
                    HirCaptureSource::Capture(_) => {}
                }
            }
        }
        HirExpressionKind::Field { base, .. }
        | HirExpressionKind::InterfaceUpcast { value: base, .. } => {
            visit_expression_closures(base, parameters, locals);
        }
        HirExpressionKind::ArrayGet { array, index }
        | HirExpressionKind::ArrayGetChecked { array, index }
        | HirExpressionKind::Binary {
            left: array,
            right: index,
            ..
        } => {
            visit_expression_closures(array, parameters, locals);
            visit_expression_closures(index, parameters, locals);
        }
        HirExpressionKind::ArrayCreate {
            length,
            initial_value,
        } => {
            visit_expression_closures(length, parameters, locals);
            visit_expression_closures(initial_value, parameters, locals);
        }
        HirExpressionKind::ArrayLength { array } => {
            visit_expression_closures(array, parameters, locals);
        }
        HirExpressionKind::ArrayFill { array, value } => {
            visit_expression_closures(array, parameters, locals);
            visit_expression_closures(value, parameters, locals);
        }
        HirExpressionKind::Record { fields, .. }
        | HirExpressionKind::ClassConstruct { fields, .. } => {
            for field in fields {
                visit_expression_closures(field.value(), parameters, locals);
            }
        }
        HirExpressionKind::RecordUpdate { base, fields, .. } => {
            visit_expression_closures(base, parameters, locals);
            for field in fields {
                visit_expression_closures(field.value(), parameters, locals);
            }
        }
        HirExpressionKind::Array(elements)
        | HirExpressionKind::Tuple(elements)
        | HirExpressionKind::UnionCase {
            arguments: elements,
            ..
        } => {
            for element in elements {
                visit_expression_closures(element, parameters, locals);
            }
        }
        HirExpressionKind::Table(entries) => {
            for entry in entries {
                visit_expression_closures(entry.key(), parameters, locals);
                visit_expression_closures(entry.value(), parameters, locals);
            }
        }
        HirExpressionKind::Unary { operand, .. } => {
            visit_expression_closures(operand, parameters, locals);
        }
        HirExpressionKind::Call { arguments, .. } => {
            for argument in arguments {
                visit_expression_closures(argument, parameters, locals);
            }
        }
        HirExpressionKind::Integer(_)
        | HirExpressionKind::Float(_)
        | HirExpressionKind::String(_)
        | HirExpressionKind::Boolean(_)
        | HirExpressionKind::Nil
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Parameter(_)
        | HirExpressionKind::Capture(_)
        | HirExpressionKind::Function(_) => {}
    }
}

impl<'hir> FunctionBuilder<'hir> {
    fn new(
        hir: &'hir HirFunction,
        arena: &'hir TypeArena,
        gc_schema: &'hir LoweringGcSchema,
    ) -> Self {
        let parameter_specs: Vec<_> = hir
            .parameters()
            .iter()
            .map(|parameter| (parameter.parameter(), parameter.type_id(), parameter.span()))
            .collect();
        Self::from_parts(
            hir.symbol(),
            parameter_specs,
            hir.results().to_vec(),
            hir.body(),
            BTreeMap::new(),
            arena,
            gc_schema,
        )
    }

    fn new_closure(
        owner: SymbolId,
        closure: &'hir HirClosure,
        arena: &'hir TypeArena,
        gc_schema: &'hir LoweringGcSchema,
    ) -> Self {
        let parameter_specs = closure
            .parameters()
            .iter()
            .map(|parameter| (parameter.parameter(), parameter.type_id(), parameter.span()))
            .collect();
        let capture_schema = closure
            .captures()
            .iter()
            .enumerate()
            .map(|(slot, capture)| {
                (
                    capture.capture(),
                    MirCapture {
                        capture: capture.capture(),
                        binding: capture.binding(),
                        slot: u32::try_from(slot).unwrap_or(u32::MAX),
                        type_id: capture.type_id(),
                        mode: match capture.mode() {
                            HirCaptureMode::Value => MirCaptureMode::Value,
                            HirCaptureMode::Cell => MirCaptureMode::Cell,
                        },
                    },
                )
            })
            .collect();
        Self::from_parts(
            owner,
            parameter_specs,
            closure.results().to_vec(),
            closure.body(),
            capture_schema,
            arena,
            gc_schema,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        owner: SymbolId,
        parameter_specs: Vec<(ValueParameterId, TypeId, SourceSpan)>,
        results: Vec<TypeId>,
        body: &'hir [HirStatement],
        capture_schema: BTreeMap<CaptureId, MirCapture>,
        arena: &'hir TypeArena,
        gc_schema: &'hir LoweringGcSchema,
    ) -> Self {
        let mut arguments = Vec::new();
        let mut parameters = BTreeMap::new();
        for (parameter, type_id, span) in &parameter_specs {
            let value = ValueId::from_raw(parameter.raw());
            parameters.insert(*parameter, value);
            arguments.push(MirBlockArgument {
                value,
                type_id: *type_id,
                span: *span,
            });
        }
        let (cell_parameters, cell_locals) = collect_cell_sources(body);
        Self {
            owner,
            parameters_schema: parameter_specs
                .iter()
                .map(|(_, type_id, _)| *type_id)
                .collect(),
            results,
            body,
            capture_schema,
            arena,
            gc_schema,
            blocks: vec![BuildingBlock {
                arguments,
                instructions: Vec::new(),
                terminator: MirTerminator::Missing,
            }],
            current: BlockId::from_raw(0),
            next_value: u32::try_from(parameter_specs.len()).unwrap_or(u32::MAX),
            parameters,
            locals: BTreeMap::new(),
            parameter_cells: BTreeMap::new(),
            local_cells: BTreeMap::new(),
            cell_parameters,
            cell_locals,
            nested_functions: Vec::new(),
        }
    }

    fn lower(mut self) -> (MirFunction, Vec<MirNestedFunction>) {
        self.initialize_parameter_cells();
        self.lower_statements(self.body);
        for block in &mut self.blocks {
            if matches!(block.terminator, MirTerminator::Missing) {
                block.terminator = if self.results.is_empty() {
                    MirTerminator::Return { values: Vec::new() }
                } else {
                    MirTerminator::Unreachable
                };
            }
        }
        let blocks = self
            .blocks
            .into_iter()
            .enumerate()
            .map(|(index, block)| MirBlock {
                block: BlockId::from_raw(u32::try_from(index).unwrap_or(u32::MAX)),
                arguments: block.arguments,
                instructions: block.instructions,
                terminator: block.terminator,
            })
            .collect();
        let function = MirFunction {
            function: FunctionId::from_raw(0),
            symbol: self.owner,
            parameters: self.parameters_schema,
            results: self.results,
            effects: MirEffectSummary::empty(),
            effects_explicit: false,
            blocks,
        };
        (function, self.nested_functions)
    }

    fn initialize_parameter_cells(&mut self) {
        let parameters: Vec<_> = self.cell_parameters.iter().copied().collect();
        for parameter in parameters {
            let initial = self.parameters[&parameter];
            let type_id = self.parameters_schema[parameter.raw() as usize];
            let cell = self.emit(
                MirInstructionKind::CaptureCellAllocate {
                    binding: BindingId::from_raw(parameter.raw()),
                    initial,
                    value_type: type_id,
                    object_map: capture_cell_object_map(self.arena, type_id),
                },
                type_id,
                SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0))),
            );
            self.parameter_cells.insert(parameter, cell);
        }
    }

    fn lower_statements(&mut self, statements: &[HirStatement]) {
        for statement in statements {
            if !matches!(self.current_block().terminator, MirTerminator::Missing) {
                self.current = self.new_block();
            }
            match statement.kind() {
                HirStatementKind::Local {
                    binding,
                    local,
                    local_type,
                    initializer,
                    ..
                } => {
                    let value = self.lower_expression(initializer);
                    if self.cell_locals.contains(local) {
                        let cell = self.emit(
                            MirInstructionKind::CaptureCellAllocate {
                                binding: *binding,
                                initial: value,
                                value_type: *local_type,
                                object_map: capture_cell_object_map(self.arena, *local_type),
                            },
                            *local_type,
                            statement.span(),
                        );
                        self.local_cells.insert(*local, cell);
                    } else {
                        self.locals.insert(*local, value);
                    }
                }
                HirStatementKind::LocalSet { local, value } => {
                    let value = self.lower_expression(value);
                    if let Some(cell) = self.local_cells.get(local).copied() {
                        self.emit_effect(
                            MirInstructionKind::CaptureCellStore { cell, value },
                            statement.span(),
                        );
                    } else {
                        self.locals.insert(*local, value);
                    }
                }
                HirStatementKind::ParameterSet { parameter, value } => {
                    let value = self.lower_expression(value);
                    if let Some(cell) = self.parameter_cells.get(parameter).copied() {
                        self.emit_effect(
                            MirInstructionKind::CaptureCellStore { cell, value },
                            statement.span(),
                        );
                    } else {
                        self.parameters.insert(*parameter, value);
                    }
                }
                HirStatementKind::CaptureSet { capture, value } => {
                    let value = self.lower_expression(value);
                    let schema = self.capture_schema[capture];
                    self.emit_effect(
                        MirInstructionKind::CaptureStore {
                            capture: *capture,
                            slot: schema.slot(),
                            value,
                        },
                        statement.span(),
                    );
                }
                HirStatementKind::Return { values } => {
                    let values = values
                        .iter()
                        .map(|value| self.lower_expression(value))
                        .collect();
                    self.terminate(MirTerminator::Return { values });
                }
                HirStatementKind::If {
                    condition,
                    then_body,
                    else_body,
                } => self.lower_if(condition, then_body, else_body),
                HirStatementKind::While { condition, body } => {
                    self.lower_while(condition, body);
                }
                HirStatementKind::RepeatUntil { body, condition } => {
                    self.lower_repeat_until(body, condition);
                }
                HirStatementKind::Match {
                    scrutinee,
                    union,
                    arms,
                } => {
                    self.lower_match(scrutinee, *union, arms);
                }
                HirStatementKind::FieldSet { base, field, value } => {
                    let base = self.lower_expression(base);
                    let value = self.lower_expression(value);
                    if let Some((slot, field_type)) = self.gc_schema.fields.get(field).copied()
                        && is_managed_reference_type_id(field_type, Some(self.arena))
                    {
                        let previous = self.emit(
                            MirInstructionKind::FieldGet {
                                base,
                                field: *field,
                            },
                            field_type,
                            statement.span(),
                        );
                        self.emit_effect(
                            MirInstructionKind::WriteBarrier {
                                owner: base,
                                slot,
                                previous: Some(previous),
                                value: Some(value),
                            },
                            statement.span(),
                        );
                    }
                    let nil = self
                        .arena
                        .source_type("nil")
                        .expect("validated type arena always contains nil");
                    self.emit(
                        MirInstructionKind::FieldSet {
                            base,
                            field: *field,
                            value,
                        },
                        nil,
                        statement.span(),
                    );
                }
                HirStatementKind::ArraySet {
                    array,
                    index,
                    value,
                } => {
                    let element_map = array_element_map(self.arena, array.type_id());
                    let array = self.lower_expression(array);
                    let index = self.lower_expression(index);
                    let value = self.lower_expression(value);
                    let nil = self
                        .arena
                        .source_type("nil")
                        .expect("validated type arena always contains nil");
                    self.emit(
                        MirInstructionKind::ArraySet {
                            array,
                            index,
                            value,
                            element_map,
                        },
                        nil,
                        statement.span(),
                    );
                }
                HirStatementKind::Call(call) => {
                    let kind = self.lower_call(call.dispatch(), call.arguments());
                    self.emit_effect(kind, call.span());
                }
                HirStatementKind::Expression(expression) => {
                    self.lower_expression(expression);
                }
            }
        }
    }

    fn lower_match(&mut self, scrutinee: &HirExpression, union: SymbolId, arms: &[HirMatchArm]) {
        let scrutinee = self.lower_expression(scrutinee);
        let dispatch_block = self.current;
        let join = self.new_block();
        let outer_locals = self.locals.clone();
        let mut switch_arms = Vec::new();
        for arm in arms {
            let specs: Vec<_> = arm
                .bindings()
                .iter()
                .map(|binding| (binding.type_id(), binding.span()))
                .collect();
            let (block, arguments) = self.new_block_with_arguments(&specs);
            switch_arms.push(MirUnionSwitchArm {
                case: arm.case(),
                target: block,
            });
            self.current = block;
            self.locals.clone_from(&outer_locals);
            for (binding, argument) in arm.bindings().iter().zip(arguments) {
                if let Some(local) = binding.local() {
                    self.locals.insert(local, argument);
                }
            }
            self.lower_statements(arm.body());
            self.branch_if_open(join);
        }
        self.locals = outer_locals;
        self.current = dispatch_block;
        self.terminate(MirTerminator::UnionSwitch {
            scrutinee,
            union,
            arms: switch_arms,
        });
        self.current = join;
    }

    fn lower_if(
        &mut self,
        condition: &HirExpression,
        then_body: &[HirStatement],
        else_body: &[HirStatement],
    ) {
        let condition_span = condition.span();
        let condition = self.lower_expression(condition);
        let state = self.live_state(condition_span);
        let then_block = self.new_block();
        let else_block = self.new_block();
        let (join_block, join_arguments) = self.new_block_with_arguments(&state.specs);
        self.terminate(MirTerminator::ConditionalBranch {
            condition,
            when_true: then_block,
            when_false: else_block,
        });
        let outer_parameters = self.parameters.clone();
        let outer_locals = self.locals.clone();
        self.current = then_block;
        self.lower_statements(then_body);
        self.branch_with_state_if_open(join_block, &state);
        self.parameters.clone_from(&outer_parameters);
        self.locals.clone_from(&outer_locals);
        self.current = else_block;
        self.lower_statements(else_body);
        self.branch_with_state_if_open(join_block, &state);
        self.current = join_block;
        self.install_state(&state, &join_arguments);
    }

    fn lower_while(&mut self, condition: &HirExpression, body: &[HirStatement]) {
        let state = self.live_state(condition.span());
        let initial_values = self.state_values(&state);
        let (condition_block, condition_arguments) = self.new_block_with_arguments(&state.specs);
        let body_block = self.new_block();
        let exit_edge = self.new_block();
        let (exit_block, exit_arguments) = self.new_block_with_arguments(&state.specs);
        self.branch_with_arguments_if_open(condition_block, initial_values);
        self.current = condition_block;
        self.install_state(&state, &condition_arguments);
        let condition = self.lower_expression(condition);
        self.terminate(MirTerminator::ConditionalBranch {
            condition,
            when_true: body_block,
            when_false: exit_edge,
        });
        self.current = body_block;
        self.lower_statements(body);
        self.branch_with_state_if_open(condition_block, &state);
        self.current = exit_edge;
        self.install_state(&state, &condition_arguments);
        self.branch_with_state_if_open(exit_block, &state);
        self.current = exit_block;
        self.install_state(&state, &exit_arguments);
    }

    fn lower_repeat_until(&mut self, body: &[HirStatement], condition: &HirExpression) {
        let state = self.live_state(condition.span());
        let initial_values = self.state_values(&state);
        let outer_locals = self.locals.clone();
        let (body_block, body_arguments) = self.new_block_with_arguments(&state.specs);
        let condition_block = self.new_block();
        let repeat_edge = self.new_block();
        let exit_edge = self.new_block();
        let (exit_block, exit_arguments) = self.new_block_with_arguments(&state.specs);

        self.branch_with_arguments_if_open(body_block, initial_values);
        self.current = body_block;
        self.install_state(&state, &body_arguments);
        self.lower_statements(body);
        self.branch_if_open(condition_block);

        self.current = condition_block;
        let condition = self.lower_expression(condition);
        self.terminate(MirTerminator::ConditionalBranch {
            condition,
            when_true: exit_edge,
            when_false: repeat_edge,
        });

        self.current = repeat_edge;
        self.branch_with_state_if_open(body_block, &state);

        self.current = exit_edge;
        self.branch_with_state_if_open(exit_block, &state);

        self.current = exit_block;
        self.locals = outer_locals;
        self.install_state(&state, &exit_arguments);
    }

    #[allow(clippy::too_many_lines)]
    fn lower_expression(&mut self, expression: &HirExpression) -> ValueId {
        let kind = match expression.kind() {
            HirExpressionKind::Integer(value) => MirInstructionKind::IntegerConstant(*value),
            HirExpressionKind::Float(value) => MirInstructionKind::FloatConstant(*value),
            HirExpressionKind::String(value) => MirInstructionKind::StringConstant(unquote(value)),
            HirExpressionKind::Boolean(value) => MirInstructionKind::BooleanConstant(*value),
            HirExpressionKind::Nil => MirInstructionKind::NilConstant,
            HirExpressionKind::Closure(closure) => {
                return self.lower_closure(closure, expression.type_id());
            }
            HirExpressionKind::Local(local) => {
                if let Some(cell) = self.local_cells.get(local).copied() {
                    return self.emit(
                        MirInstructionKind::CaptureCellLoad { cell },
                        expression.type_id(),
                        expression.span(),
                    );
                }
                return self.locals[local];
            }
            HirExpressionKind::Parameter(parameter) => {
                if let Some(cell) = self.parameter_cells.get(parameter).copied() {
                    return self.emit(
                        MirInstructionKind::CaptureCellLoad { cell },
                        expression.type_id(),
                        expression.span(),
                    );
                }
                return self.parameters[parameter];
            }
            HirExpressionKind::Capture(capture) => {
                let schema = self.capture_schema[capture];
                return self.emit(
                    MirInstructionKind::CaptureLoad {
                        capture: *capture,
                        slot: schema.slot(),
                        mode: schema.mode(),
                    },
                    expression.type_id(),
                    expression.span(),
                );
            }
            HirExpressionKind::Function(function) => {
                MirInstructionKind::FunctionReference(*function)
            }
            HirExpressionKind::Tuple(elements) => MirInstructionKind::TupleMake(
                elements
                    .iter()
                    .map(|element| self.lower_expression(element))
                    .collect(),
            ),
            HirExpressionKind::Array(elements) => MirInstructionKind::ArrayMake {
                elements: elements
                    .iter()
                    .map(|element| self.lower_expression(element))
                    .collect(),
                element_map: array_element_map(self.arena, expression.type_id()),
            },
            HirExpressionKind::ArrayCreate {
                length,
                initial_value,
            } => MirInstructionKind::ArrayCreate {
                length: self.lower_expression(length),
                initial_value: self.lower_expression(initial_value),
                element_map: array_element_map(self.arena, expression.type_id()),
            },
            HirExpressionKind::Table(entries) => {
                let (key_map, value_map) = table_element_maps(self.arena, expression.type_id());
                MirInstructionKind::TableMake {
                    key_map,
                    value_map,
                    entries: self.lower_table_entries(entries),
                }
            }
            HirExpressionKind::Unary { operator, operand } => {
                let operand = self.lower_expression(operand);
                match operator {
                    TypedUnaryOperator::Not => MirInstructionKind::BooleanNot { operand },
                    TypedUnaryOperator::Negate => {
                        if let Some(kind) = integer_kind(self.arena, expression.type_id()) {
                            MirInstructionKind::IntegerNegate { kind, operand }
                        } else {
                            MirInstructionKind::FloatNegate {
                                kind: float_kind(self.arena, expression.type_id())
                                    .expect("typed numeric negation has a numeric type"),
                                operand,
                            }
                        }
                    }
                }
            }
            HirExpressionKind::Binary {
                operator,
                left,
                right,
            } => return self.lower_binary_expression(expression, *operator, left, right),
            HirExpressionKind::Call {
                dispatch,
                arguments,
            } => self.lower_call(dispatch, arguments),
            HirExpressionKind::InterfaceUpcast { value, interface } => {
                let value = self.lower_expression(value);
                MirInstructionKind::InterfaceUpcast {
                    value,
                    interface: *interface,
                }
            }
            HirExpressionKind::Field { base, field } => MirInstructionKind::FieldGet {
                base: self.lower_expression(base),
                field: *field,
            },
            HirExpressionKind::ArrayGet { array, index } => {
                let array = self.lower_expression(array);
                let index = self.lower_expression(index);
                MirInstructionKind::ArrayGet { array, index }
            }
            HirExpressionKind::ArrayLength { array } => MirInstructionKind::ArrayLength {
                array: self.lower_expression(array),
            },
            HirExpressionKind::ArrayGetChecked { array, index } => {
                MirInstructionKind::ArrayGetChecked {
                    array: self.lower_expression(array),
                    index: self.lower_expression(index),
                }
            }
            HirExpressionKind::ArrayFill { array, value } => MirInstructionKind::ArrayFill {
                array: self.lower_expression(array),
                value: self.lower_expression(value),
                element_map: array_element_map(self.arena, array.type_id()),
            },
            HirExpressionKind::Record { record, fields } => MirInstructionKind::RecordMake {
                record: *record,
                fields: self.lower_fields(fields),
            },
            HirExpressionKind::ClassConstruct { class, fields, .. } => {
                MirInstructionKind::ClassMake {
                    class: *class,
                    fields: self.lower_fields(fields),
                    object_map: self
                        .gc_schema
                        .classes
                        .get(class)
                        .cloned()
                        .expect("verified class construction has a GC schema"),
                }
            }
            HirExpressionKind::RecordUpdate {
                record,
                base,
                fields,
            } => {
                let base = self.lower_expression(base);
                MirInstructionKind::RecordUpdate {
                    record: *record,
                    base,
                    fields: self.lower_fields(fields),
                }
            }
            HirExpressionKind::UnionCase {
                union,
                case,
                arguments,
            } => MirInstructionKind::UnionMake {
                union: *union,
                case: *case,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
            },
        };
        self.emit(kind, expression.type_id(), expression.span())
    }

    fn lower_closure(&mut self, closure: &HirClosure, closure_type: TypeId) -> ValueId {
        let (lowered, mut nested) =
            FunctionBuilder::new_closure(self.owner, closure, self.arena, self.gc_schema).lower();
        let captures: Vec<_> = closure
            .captures()
            .iter()
            .enumerate()
            .map(|(slot, capture)| {
                let self_reference = matches!(
                    (capture.source(), capture.mode()),
                    (HirCaptureSource::Local(local), HirCaptureMode::Cell)
                        if !self.local_cells.contains_key(&local)
                );
                let value = if self_reference {
                    ValueId::from_raw(u32::MAX)
                } else {
                    self.lower_capture_source(
                        capture.source(),
                        capture.mode(),
                        capture.type_id(),
                        closure.span(),
                    )
                };
                MirClosureCapture {
                    capture: capture.capture(),
                    binding: capture.binding(),
                    slot: u32::try_from(slot).unwrap_or(u32::MAX),
                    value,
                    self_reference,
                    type_id: capture.type_id(),
                    mode: match capture.mode() {
                        HirCaptureMode::Value => MirCaptureMode::Value,
                        HirCaptureMode::Cell => MirCaptureMode::Cell,
                    },
                }
            })
            .collect();
        let object_map = closure_environment_object_map(self.arena, &captures);
        self.nested_functions.push(MirNestedFunction {
            owner: self.owner,
            function: closure.function(),
            captures: closure
                .captures()
                .iter()
                .enumerate()
                .map(|(slot, capture)| MirCapture {
                    capture: capture.capture(),
                    binding: capture.binding(),
                    slot: u32::try_from(slot).unwrap_or(u32::MAX),
                    type_id: capture.type_id(),
                    mode: match capture.mode() {
                        HirCaptureMode::Value => MirCaptureMode::Value,
                        HirCaptureMode::Cell => MirCaptureMode::Cell,
                    },
                })
                .collect(),
            parameters: lowered.parameters,
            results: lowered.results,
            effects: lowered.effects,
            effects_explicit: lowered.effects_explicit,
            blocks: lowered.blocks,
        });
        self.nested_functions.append(&mut nested);
        self.emit(
            MirInstructionKind::ClosureEnvironmentAllocate {
                owner: self.owner,
                function: closure.function(),
                captures,
                object_map,
            },
            closure_type,
            closure.span(),
        )
    }

    fn lower_capture_source(
        &mut self,
        source: HirCaptureSource,
        mode: HirCaptureMode,
        type_id: TypeId,
        span: SourceSpan,
    ) -> ValueId {
        match (source, mode) {
            (HirCaptureSource::Local(local), HirCaptureMode::Cell) => self.local_cells[&local],
            (HirCaptureSource::Parameter(parameter), HirCaptureMode::Cell) => {
                self.parameter_cells[&parameter]
            }
            (HirCaptureSource::Capture(capture), HirCaptureMode::Cell) => {
                let schema = self.capture_schema[&capture];
                self.emit(
                    MirInstructionKind::CaptureCellReference {
                        capture,
                        slot: schema.slot(),
                    },
                    type_id,
                    span,
                )
            }
            (HirCaptureSource::Local(local), HirCaptureMode::Value) => {
                if let Some(cell) = self.local_cells.get(&local).copied() {
                    self.emit(MirInstructionKind::CaptureCellLoad { cell }, type_id, span)
                } else {
                    self.locals[&local]
                }
            }
            (HirCaptureSource::Parameter(parameter), HirCaptureMode::Value) => {
                if let Some(cell) = self.parameter_cells.get(&parameter).copied() {
                    self.emit(MirInstructionKind::CaptureCellLoad { cell }, type_id, span)
                } else {
                    self.parameters[&parameter]
                }
            }
            (HirCaptureSource::Capture(capture), HirCaptureMode::Value) => {
                let schema = self.capture_schema[&capture];
                self.emit(
                    MirInstructionKind::CaptureLoad {
                        capture,
                        slot: schema.slot(),
                        mode: schema.mode(),
                    },
                    type_id,
                    span,
                )
            }
        }
    }

    fn lower_binary_expression(
        &mut self,
        expression: &HirExpression,
        operator: TypedBinaryOperator,
        left: &HirExpression,
        right: &HirExpression,
    ) -> ValueId {
        if matches!(operator, TypedBinaryOperator::And | TypedBinaryOperator::Or) {
            return self.lower_short_circuit(
                operator,
                left,
                right,
                expression.type_id(),
                expression.span(),
            );
        }
        let operand_type = left.type_id();
        let left = self.lower_expression(left);
        let right = self.lower_expression(right);
        self.emit(
            lower_binary(self.arena, operator, operand_type, left, right),
            expression.type_id(),
            expression.span(),
        )
    }

    fn lower_short_circuit(
        &mut self,
        operator: TypedBinaryOperator,
        left: &HirExpression,
        right: &HirExpression,
        result_type: TypeId,
        span: SourceSpan,
    ) -> ValueId {
        let left = self.lower_expression(left);
        let right_block = self.new_block();
        let short_block = self.new_block();
        let (join_block, result) = self.new_block_with_argument(result_type, span);
        let (when_true, when_false, short_value) = match operator {
            TypedBinaryOperator::And => (right_block, short_block, false),
            TypedBinaryOperator::Or => (short_block, right_block, true),
            _ => unreachable!("short-circuit lowering accepts only logical operators"),
        };
        self.terminate(MirTerminator::ConditionalBranch {
            condition: left,
            when_true,
            when_false,
        });

        self.current = short_block;
        let short_value = self.emit(
            MirInstructionKind::BooleanConstant(short_value),
            result_type,
            span,
        );
        self.terminate(MirTerminator::Branch {
            target: join_block,
            arguments: vec![short_value],
        });

        self.current = right_block;
        let right = self.lower_expression(right);
        self.terminate(MirTerminator::Branch {
            target: join_block,
            arguments: vec![right],
        });
        self.current = join_block;
        result
    }

    fn lower_fields(&mut self, fields: &[HirFieldValue]) -> Vec<(FieldId, ValueId)> {
        fields
            .iter()
            .map(|field| (field.field(), self.lower_expression(field.value())))
            .collect()
    }

    fn lower_call(
        &mut self,
        dispatch: &HirCallDispatch,
        arguments: &[HirExpression],
    ) -> MirInstructionKind {
        match dispatch {
            HirCallDispatch::Standard { function } => MirInstructionKind::CallStandard {
                function: *function,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
                declared_effects: MirEffectSummary::empty().with(MirEffect::AmbientIo),
            },
            HirCallDispatch::Direct { function } => MirInstructionKind::CallDirect {
                function: *function,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
                declared_effects: MirEffectSummary::empty(),
                unwind: MirUnwindAction::Propagate,
            },
            HirCallDispatch::DirectMethod { method } => MirInstructionKind::CallDirectMethod {
                method: *method,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
                declared_effects: MirEffectSummary::empty(),
                unwind: MirUnwindAction::Propagate,
            },
            HirCallDispatch::InterfaceMethod {
                interface,
                method,
                slot,
            } => MirInstructionKind::CallInterface {
                interface: *interface,
                method: *method,
                slot: *slot,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
                declared_effects: conservative_indirect_effects(),
                unwind: MirUnwindAction::Propagate,
            },
            HirCallDispatch::Indirect { callee } => {
                let callee = self.lower_expression(callee);
                MirInstructionKind::CallIndirect {
                    callee,
                    arguments: arguments
                        .iter()
                        .map(|argument| self.lower_expression(argument))
                        .collect(),
                    declared_effects: conservative_indirect_effects(),
                    unwind: MirUnwindAction::Propagate,
                }
            }
        }
    }

    fn lower_table_entries(&mut self, entries: &[HirTableEntry]) -> Vec<(ValueId, ValueId)> {
        let mut lowered = Vec::with_capacity(entries.len());
        for entry in entries {
            let key = self.lower_expression(entry.key());
            let value = self.lower_expression(entry.value());
            lowered.push((key, value));
        }
        lowered
    }

    fn emit(&mut self, kind: MirInstructionKind, type_id: TypeId, span: SourceSpan) -> ValueId {
        let value = ValueId::from_raw(self.next_value);
        self.next_value = self.next_value.saturating_add(1);
        let effects = local_instruction_effects(&kind);
        self.current_block_mut().instructions.push(MirInstruction {
            result: value,
            result_type: Some(type_id),
            kind,
            effects,
            effects_explicit: false,
            span,
        });
        value
    }

    fn emit_effect(&mut self, kind: MirInstructionKind, span: SourceSpan) {
        let instruction = ValueId::from_raw(self.next_value);
        self.next_value = self.next_value.saturating_add(1);
        let effects = local_instruction_effects(&kind);
        self.current_block_mut().instructions.push(MirInstruction {
            result: instruction,
            result_type: None,
            kind,
            effects,
            effects_explicit: false,
            span,
        });
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId::from_raw(u32::try_from(self.blocks.len()).unwrap_or(u32::MAX));
        self.blocks.push(BuildingBlock {
            arguments: Vec::new(),
            instructions: Vec::new(),
            terminator: MirTerminator::Missing,
        });
        id
    }

    fn new_block_with_argument(&mut self, type_id: TypeId, span: SourceSpan) -> (BlockId, ValueId) {
        let block = self.new_block();
        let value = ValueId::from_raw(self.next_value);
        self.next_value = self.next_value.saturating_add(1);
        self.blocks[block.raw() as usize]
            .arguments
            .push(MirBlockArgument {
                value,
                type_id,
                span,
            });
        (block, value)
    }

    fn new_block_with_arguments(
        &mut self,
        arguments: &[(TypeId, SourceSpan)],
    ) -> (BlockId, Vec<ValueId>) {
        let block = self.new_block();
        let mut values = Vec::with_capacity(arguments.len());
        for (type_id, span) in arguments {
            let value = ValueId::from_raw(self.next_value);
            self.next_value = self.next_value.saturating_add(1);
            self.blocks[block.raw() as usize]
                .arguments
                .push(MirBlockArgument {
                    value,
                    type_id: *type_id,
                    span: *span,
                });
            values.push(value);
        }
        (block, values)
    }

    fn live_state(&self, span: SourceSpan) -> LiveState {
        let parameters = self
            .parameters
            .keys()
            .filter(|parameter| !self.parameter_cells.contains_key(parameter))
            .copied()
            .collect::<Vec<_>>();
        let locals = self
            .locals
            .keys()
            .filter(|local| !self.local_cells.contains_key(local))
            .copied()
            .collect::<Vec<_>>();
        let specs = parameters
            .iter()
            .map(|parameter| self.parameters[parameter])
            .chain(locals.iter().map(|local| self.locals[local]))
            .map(|value| (self.value_type(value), span))
            .collect();
        LiveState {
            parameters,
            locals,
            specs,
        }
    }

    fn state_values(&self, state: &LiveState) -> Vec<ValueId> {
        state
            .parameters
            .iter()
            .map(|parameter| self.parameters[parameter])
            .chain(state.locals.iter().map(|local| self.locals[local]))
            .collect()
    }

    fn install_state(&mut self, state: &LiveState, values: &[ValueId]) {
        let parameter_count = state.parameters.len();
        for (parameter, value) in state.parameters.iter().zip(values.iter().copied()) {
            self.parameters.insert(*parameter, value);
        }
        for (local, value) in state
            .locals
            .iter()
            .zip(values[parameter_count..].iter().copied())
        {
            self.locals.insert(*local, value);
        }
    }

    fn value_type(&self, value: ValueId) -> TypeId {
        self.blocks
            .iter()
            .flat_map(|block| block.arguments.iter())
            .find(|argument| argument.value == value)
            .map(|argument| argument.type_id)
            .or_else(|| {
                self.blocks
                    .iter()
                    .flat_map(|block| block.instructions.iter())
                    .find(|instruction| instruction.result == value)
                    .and_then(|instruction| instruction.result_type)
            })
            .expect("lowered live state always has a statically proven MIR type")
    }

    fn current_block(&self) -> &BuildingBlock {
        &self.blocks[self.current.raw() as usize]
    }

    fn current_block_mut(&mut self) -> &mut BuildingBlock {
        &mut self.blocks[self.current.raw() as usize]
    }

    fn terminate(&mut self, terminator: MirTerminator) {
        self.current_block_mut().terminator = terminator;
    }

    fn branch_if_open(&mut self, target: BlockId) {
        if matches!(self.current_block().terminator, MirTerminator::Missing) {
            self.terminate(MirTerminator::Branch {
                target,
                arguments: Vec::new(),
            });
        }
    }

    fn branch_with_arguments_if_open(&mut self, target: BlockId, arguments: Vec<ValueId>) {
        if matches!(self.current_block().terminator, MirTerminator::Missing) {
            self.terminate(MirTerminator::Branch { target, arguments });
        }
    }

    fn branch_with_state_if_open(&mut self, target: BlockId, state: &LiveState) {
        self.branch_with_arguments_if_open(target, self.state_values(state));
    }
}

fn lower_binary(
    arena: &TypeArena,
    operator: TypedBinaryOperator,
    operand_type: TypeId,
    left: ValueId,
    right: ValueId,
) -> MirInstructionKind {
    match operator {
        TypedBinaryOperator::Or => MirInstructionKind::BooleanOr { left, right },
        TypedBinaryOperator::And => MirInstructionKind::BooleanAnd { left, right },
        TypedBinaryOperator::Equal => MirInstructionKind::CompareEqual { left, right },
        TypedBinaryOperator::NotEqual => MirInstructionKind::CompareNotEqual { left, right },
        TypedBinaryOperator::LessThan => {
            if let Some(kind) = integer_kind(arena, operand_type) {
                MirInstructionKind::CompareIntegerLess { kind, left, right }
            } else {
                MirInstructionKind::CompareFloatLess {
                    kind: float_kind(arena, operand_type)
                        .expect("typed comparison has numeric operands"),
                    left,
                    right,
                }
            }
        }
        TypedBinaryOperator::GreaterThan => {
            if let Some(kind) = integer_kind(arena, operand_type) {
                MirInstructionKind::CompareIntegerGreater { kind, left, right }
            } else {
                MirInstructionKind::CompareFloatGreater {
                    kind: float_kind(arena, operand_type)
                        .expect("typed comparison has numeric operands"),
                    left,
                    right,
                }
            }
        }
        TypedBinaryOperator::Add => numeric_binary(
            arena,
            operand_type,
            left,
            right,
            |kind, left, right| MirInstructionKind::CheckedIntegerAdd { kind, left, right },
            |kind, left, right| MirInstructionKind::FloatAdd { kind, left, right },
        ),
        TypedBinaryOperator::Subtract => numeric_binary(
            arena,
            operand_type,
            left,
            right,
            |kind, left, right| MirInstructionKind::CheckedIntegerSubtract { kind, left, right },
            |kind, left, right| MirInstructionKind::FloatSubtract { kind, left, right },
        ),
        TypedBinaryOperator::Multiply => numeric_binary(
            arena,
            operand_type,
            left,
            right,
            |kind, left, right| MirInstructionKind::CheckedIntegerMultiply { kind, left, right },
            |kind, left, right| MirInstructionKind::FloatMultiply { kind, left, right },
        ),
        TypedBinaryOperator::Divide => numeric_binary(
            arena,
            operand_type,
            left,
            right,
            |kind, left, right| MirInstructionKind::CheckedIntegerDivide { kind, left, right },
            |kind, left, right| MirInstructionKind::FloatDivide { kind, left, right },
        ),
        TypedBinaryOperator::Remainder => MirInstructionKind::CheckedIntegerRemainder {
            kind: integer_kind(arena, operand_type).expect("typed remainder has integer operands"),
            left,
            right,
        },
    }
}

fn numeric_binary(
    arena: &TypeArena,
    operand_type: TypeId,
    left: ValueId,
    right: ValueId,
    integer: impl FnOnce(IntegerKind, ValueId, ValueId) -> MirInstructionKind,
    float: impl FnOnce(FloatKind, ValueId, ValueId) -> MirInstructionKind,
) -> MirInstructionKind {
    if let Some(kind) = integer_kind(arena, operand_type) {
        integer(kind, left, right)
    } else {
        float(
            float_kind(arena, operand_type).expect("typed arithmetic has numeric operands"),
            left,
            right,
        )
    }
}

fn integer_kind(arena: &TypeArena, type_id: TypeId) -> Option<IntegerKind> {
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

fn array_element_map(arena: &TypeArena, type_id: TypeId) -> ArrayElementMap {
    match arena.get(type_id) {
        Some(SemanticType::Array(element))
            if is_managed_reference_type_id(*element, Some(arena)) =>
        {
            ArrayElementMap::ManagedReference
        }
        _ => ArrayElementMap::Scalar,
    }
}

fn table_element_maps(arena: &TypeArena, type_id: TypeId) -> (ArrayElementMap, ArrayElementMap) {
    match arena.get(type_id) {
        Some(SemanticType::Table { key, value }) => {
            (element_map(arena, *key), element_map(arena, *value))
        }
        _ => (ArrayElementMap::Scalar, ArrayElementMap::Scalar),
    }
}

fn element_map(arena: &TypeArena, type_id: TypeId) -> ArrayElementMap {
    if is_managed_reference_type_id(type_id, Some(arena)) {
        ArrayElementMap::ManagedReference
    } else {
        ArrayElementMap::Scalar
    }
}

fn capture_cell_object_map(arena: &TypeArena, value_type: TypeId) -> ObjectMap {
    let references = is_managed_reference_type_id(value_type, Some(arena))
        .then(|| ObjectSlot::new(0))
        .into_iter()
        .collect();
    ObjectMap::new(1, references).expect("one-slot capture cell map is canonical")
}

fn closure_environment_object_map(arena: &TypeArena, captures: &[MirClosureCapture]) -> ObjectMap {
    let references = captures
        .iter()
        .filter(|capture| {
            capture.mode == MirCaptureMode::Cell
                || is_managed_reference_type_id(capture.type_id, Some(arena))
        })
        .map(|capture| ObjectSlot::new(capture.slot))
        .collect();
    ObjectMap::new(
        u32::try_from(captures.len()).unwrap_or(u32::MAX),
        references,
    )
    .expect("closure captures form a valid logical object map")
}

fn is_managed_reference_type_id(type_id: TypeId, arena: Option<&TypeArena>) -> bool {
    let Some(arena) = arena else {
        return false;
    };
    matches!(
        arena.get(type_id),
        Some(
            SemanticType::Primitive(PrimitiveType::String)
                | SemanticType::Array(_)
                | SemanticType::Table { .. }
                | SemanticType::Class { .. }
                | SemanticType::Interface { .. }
                | SemanticType::Builtin { .. }
        )
    )
}

fn local_instruction_effects(kind: &MirInstructionKind) -> MirEffectSummary {
    match kind {
        MirInstructionKind::CheckedIntegerAdd { .. }
        | MirInstructionKind::CheckedIntegerSubtract { .. }
        | MirInstructionKind::CheckedIntegerMultiply { .. }
        | MirInstructionKind::CheckedIntegerDivide { .. }
        | MirInstructionKind::CheckedIntegerRemainder { .. }
        | MirInstructionKind::IntegerNegate { .. }
        | MirInstructionKind::ArrayGetChecked { .. } => {
            MirEffectSummary::empty().with(MirEffect::MayTrap)
        }
        MirInstructionKind::ArraySet { element_map, .. }
        | MirInstructionKind::ArrayFill { element_map, .. } => {
            let effects = MirEffectSummary::empty().with(MirEffect::MayTrap);
            if *element_map == ArrayElementMap::ManagedReference {
                effects.with(MirEffect::WritesManagedReference)
            } else {
                effects
            }
        }
        MirInstructionKind::ArrayMake { .. }
        | MirInstructionKind::TableMake { .. }
        | MirInstructionKind::ClassMake { .. }
        | MirInstructionKind::CaptureCellAllocate { .. }
        | MirInstructionKind::ClosureEnvironmentAllocate { .. } => {
            MirEffectSummary::from_effects([
                MirEffect::Allocates,
                MirEffect::MayUnwind,
                MirEffect::GcSafePoint,
            ])
        }
        MirInstructionKind::ArrayCreate { .. } => MirEffectSummary::from_effects([
            MirEffect::Allocates,
            MirEffect::MayTrap,
            MirEffect::MayUnwind,
            MirEffect::GcSafePoint,
        ]),
        MirInstructionKind::GcSafePoint { .. } => {
            MirEffectSummary::empty().with(MirEffect::GcSafePoint)
        }
        MirInstructionKind::RetainRoot { .. }
        | MirInstructionKind::ReleaseRoot { .. }
        | MirInstructionKind::Pin { .. }
        | MirInstructionKind::Unpin { .. } => MirEffectSummary::empty().with(MirEffect::Roots),
        MirInstructionKind::WriteBarrier { .. } => {
            MirEffectSummary::empty().with(MirEffect::WritesManagedReference)
        }
        MirInstructionKind::CaptureCellStore { .. } | MirInstructionKind::CaptureStore { .. } => {
            MirEffectSummary::from_effects([MirEffect::WritesManagedReference])
        }
        MirInstructionKind::CallDirect {
            declared_effects, ..
        }
        | MirInstructionKind::CallStandard {
            declared_effects, ..
        }
        | MirInstructionKind::CallDirectMethod {
            declared_effects, ..
        }
        | MirInstructionKind::CallInterface {
            declared_effects, ..
        }
        | MirInstructionKind::CallIndirect {
            declared_effects, ..
        } => *declared_effects,
        MirInstructionKind::IntegerConstant(_)
        | MirInstructionKind::FloatConstant(_)
        | MirInstructionKind::StringConstant(_)
        | MirInstructionKind::BooleanConstant(_)
        | MirInstructionKind::NilConstant
        | MirInstructionKind::FunctionReference(_)
        | MirInstructionKind::TupleMake(_)
        | MirInstructionKind::ArrayGet { .. }
        | MirInstructionKind::ArrayLength { .. }
        | MirInstructionKind::FloatAdd { .. }
        | MirInstructionKind::FloatSubtract { .. }
        | MirInstructionKind::FloatMultiply { .. }
        | MirInstructionKind::FloatDivide { .. }
        | MirInstructionKind::FloatNegate { .. }
        | MirInstructionKind::BooleanNot { .. }
        | MirInstructionKind::BooleanAnd { .. }
        | MirInstructionKind::BooleanOr { .. }
        | MirInstructionKind::CompareEqual { .. }
        | MirInstructionKind::CompareNotEqual { .. }
        | MirInstructionKind::CompareIntegerLess { .. }
        | MirInstructionKind::CompareIntegerGreater { .. }
        | MirInstructionKind::CompareFloatLess { .. }
        | MirInstructionKind::CompareFloatGreater { .. }
        | MirInstructionKind::RecordMake { .. }
        | MirInstructionKind::RecordUpdate { .. }
        | MirInstructionKind::FieldGet { .. }
        | MirInstructionKind::FieldSet { .. }
        | MirInstructionKind::UnionMake { .. } => MirEffectSummary::empty(),
        MirInstructionKind::InterfaceUpcast { .. }
        | MirInstructionKind::CaptureCellLoad { .. }
        | MirInstructionKind::CaptureLoad { .. }
        | MirInstructionKind::CaptureCellReference { .. } => MirEffectSummary::empty(),
    }
}

fn terminator_effects(terminator: &MirTerminator) -> MirEffectSummary {
    match terminator {
        MirTerminator::Trap(_) => MirEffectSummary::empty().with(MirEffect::MayTrap),
        MirTerminator::Panic(_) | MirTerminator::ContinueUnwind(_) => {
            MirEffectSummary::empty().with(MirEffect::MayUnwind)
        }
        MirTerminator::Missing
        | MirTerminator::Branch { .. }
        | MirTerminator::ConditionalBranch { .. }
        | MirTerminator::UnionSwitch { .. }
        | MirTerminator::Return { .. }
        | MirTerminator::Unreachable => MirEffectSummary::empty(),
    }
}

fn conservative_indirect_effects() -> MirEffectSummary {
    MirEffectSummary::from_effects([
        MirEffect::Allocates,
        MirEffect::WritesManagedReference,
        MirEffect::MayTrap,
        MirEffect::MayUnwind,
        MirEffect::Suspends,
        MirEffect::UnsafeMemory,
        MirEffect::ForeignFunction,
        MirEffect::AmbientIo,
        MirEffect::GcSafePoint,
        MirEffect::Roots,
    ])
}

fn recompute_effects(bubble: &mut MirBubble) {
    let mut function_effects: BTreeMap<_, _> = bubble
        .functions
        .iter()
        .map(|function| (function.symbol, function.effects))
        .collect();
    let mut method_effects: BTreeMap<_, _> = bubble
        .methods
        .iter()
        .map(|method| (method.method, method.function.effects))
        .collect();
    loop {
        let mut changed = false;
        for function in &mut bubble.functions {
            changed |= recompute_function_effects(function, &function_effects, &method_effects);
            function_effects.insert(function.symbol, function.effects);
        }
        for method in &mut bubble.methods {
            changed |= recompute_function_effects(
                &mut method.function,
                &function_effects,
                &method_effects,
            );
            method_effects.insert(method.method, method.function.effects);
        }
        if !changed {
            break;
        }
    }
}

fn recompute_function_effects(
    function: &mut MirFunction,
    function_effects: &BTreeMap<SymbolId, MirEffectSummary>,
    method_effects: &BTreeMap<MethodId, MirEffectSummary>,
) -> bool {
    let mut summary = MirEffectSummary::empty();
    let mut changed = false;
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            let expected = match &instruction.kind {
                MirInstructionKind::CallDirect { function, .. } => {
                    function_effects.get(function).copied().unwrap_or_default()
                }
                MirInstructionKind::CallDirectMethod { method, .. } => {
                    method_effects.get(method).copied().unwrap_or_default()
                }
                MirInstructionKind::CallIndirect {
                    declared_effects, ..
                } => *declared_effects,
                _ => local_instruction_effects(&instruction.kind),
            };
            if !instruction.effects_explicit && instruction.effects != expected {
                instruction.effects = expected;
                changed = true;
            }
            if !instruction.effects_explicit {
                match &mut instruction.kind {
                    MirInstructionKind::CallDirect {
                        declared_effects, ..
                    }
                    | MirInstructionKind::CallDirectMethod {
                        declared_effects, ..
                    } => *declared_effects = expected,
                    _ => {}
                }
            }
            summary = summary.union(expected);
        }
        summary = summary.union(terminator_effects(&block.terminator));
    }
    if !function.effects_explicit && function.effects != summary {
        function.effects = summary;
        changed = true;
    }
    changed
}

fn insert_gc_safe_points(bubble: &mut MirBubble, arena: &TypeArena) -> bool {
    let mut changed = false;
    for function in &mut bubble.functions {
        changed |= insert_function_safe_points(function, arena);
    }
    for method in &mut bubble.methods {
        changed |= insert_function_safe_points(&mut method.function, arena);
    }
    changed
}

fn insert_function_safe_points(function: &mut MirFunction, arena: &TypeArena) -> bool {
    let mut next_value = function
        .blocks
        .iter()
        .flat_map(|block| {
            block.arguments.iter().map(|argument| argument.value).chain(
                block
                    .instructions
                    .iter()
                    .map(|instruction| instruction.result),
            )
        })
        .map(ValueId::raw)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut next_safe_point = function
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter())
        .filter_map(|instruction| match instruction.kind {
            MirInstructionKind::GcSafePoint { safe_point, .. } => Some(safe_point.raw()),
            _ => None,
        })
        .max()
        .map_or(0, |safe_point| safe_point.saturating_add(1));
    let mut changed = false;
    for block in &mut function.blocks {
        let mut instructions: Vec<MirInstruction> = Vec::new();
        let mut straight_line_work = 0_usize;
        for instruction in std::mem::take(&mut block.instructions) {
            let is_safe_point = matches!(instruction.kind, MirInstructionKind::GcSafePoint { .. });
            let operation_requires_safe_point = instruction.effects.contains(MirEffect::Allocates)
                || matches!(
                    instruction.kind,
                    MirInstructionKind::CallDirect { .. }
                        | MirInstructionKind::CallDirectMethod { .. }
                        | MirInstructionKind::CallInterface { .. }
                        | MirInstructionKind::CallIndirect { .. }
                ) && instruction.effects.contains(MirEffect::GcSafePoint);
            let already_at_safe_point = instructions.last().is_some_and(|previous| {
                matches!(previous.kind, MirInstructionKind::GcSafePoint { .. })
            });
            if !is_safe_point
                && !already_at_safe_point
                && (operation_requires_safe_point
                    || straight_line_work >= MAX_STRAIGHT_LINE_WORK_BETWEEN_SAFE_POINTS)
            {
                instructions.push(empty_safe_point(
                    ValueId::from_raw(next_value),
                    SafePointId::new(next_safe_point),
                    instruction.span,
                ));
                next_value = next_value.saturating_add(1);
                next_safe_point = next_safe_point.saturating_add(1);
                straight_line_work = 0;
                changed = true;
            }
            instructions.push(instruction);
            if is_safe_point {
                straight_line_work = 0;
            } else {
                straight_line_work = straight_line_work.saturating_add(1);
            }
        }
        let has_backedge = terminator_targets(&block.terminator)
            .into_iter()
            .any(|target| target <= block.block);
        if has_backedge
            && !instructions.last().is_some_and(|instruction| {
                matches!(instruction.kind, MirInstructionKind::GcSafePoint { .. })
            })
        {
            instructions.push(empty_safe_point(
                ValueId::from_raw(next_value),
                SafePointId::new(next_safe_point),
                SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0))),
            ));
            next_value = next_value.saturating_add(1);
            next_safe_point = next_safe_point.saturating_add(1);
            changed = true;
        }
        block.instructions = instructions;
    }
    populate_stack_maps(function, arena);
    changed
}

fn empty_safe_point(result: ValueId, safe_point: SafePointId, span: SourceSpan) -> MirInstruction {
    MirInstruction {
        result,
        result_type: None,
        kind: MirInstructionKind::GcSafePoint {
            safe_point,
            roots: Vec::new(),
            stack_map: StackMap::new(safe_point, Vec::new()).expect("empty stack map is canonical"),
        },
        effects: MirEffectSummary::empty().with(MirEffect::GcSafePoint),
        effects_explicit: true,
        span,
    }
}

fn populate_stack_maps(function: &mut MirFunction, arena: &TypeArena) {
    let expected = expected_safe_point_roots(function, arena);
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            let MirInstructionKind::GcSafePoint {
                safe_point,
                roots,
                stack_map,
            } = &mut instruction.kind
            else {
                continue;
            };
            *roots = expected
                .get(&instruction.result)
                .cloned()
                .unwrap_or_default();
            let root_slots = (0..roots.len())
                .map(|index| RootSlot::new(u32::try_from(index).unwrap_or(u32::MAX)))
                .collect();
            *stack_map = StackMap::new(*safe_point, root_slots)
                .expect("generated stack root slots are unique");
        }
    }
}

fn expected_safe_point_roots(
    function: &MirFunction,
    arena: &TypeArena,
) -> BTreeMap<ValueId, Vec<ValueId>> {
    let blocks: BTreeMap<_, _> = function
        .blocks
        .iter()
        .map(|block| (block.block(), block))
        .collect();
    let mut value_types = BTreeMap::new();
    for block in &function.blocks {
        for argument in &block.arguments {
            value_types.insert(argument.value, argument.type_id);
        }
        for instruction in &block.instructions {
            if let Some(type_id) = instruction.result_type {
                value_types.insert(instruction.result, type_id);
            }
        }
    }
    let mut live_in: BTreeMap<BlockId, BTreeSet<ValueId>> = function
        .blocks
        .iter()
        .map(|block| (block.block, BTreeSet::new()))
        .collect();
    let mut live_out = live_in.clone();
    loop {
        let mut changed = false;
        for block in function.blocks.iter().rev() {
            let outgoing = normal_live_out(block, &live_in, &blocks);
            let mut live = outgoing.clone();
            live.extend(terminator_operands(&block.terminator));
            for instruction in block.instructions.iter().rev() {
                if instruction.has_result() {
                    live.remove(&instruction.result);
                }
                if !matches!(instruction.kind, MirInstructionKind::GcSafePoint { .. }) {
                    if let Some(target) = instruction_unwind_target(&instruction.kind)
                        && let Some(cleanup_live) = live_in.get(&target)
                    {
                        live.extend(cleanup_live.iter().copied());
                    }
                    live.extend(instruction_operands(&instruction.kind));
                }
            }
            if live_out.get(&block.block) != Some(&outgoing) {
                live_out.insert(block.block, outgoing);
                changed = true;
            }
            if live_in.get(&block.block) != Some(&live) {
                live_in.insert(block.block, live);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut maps = BTreeMap::new();
    for block in &function.blocks {
        let mut live = live_out.get(&block.block).cloned().unwrap_or_default();
        live.extend(terminator_operands(&block.terminator));
        for instruction in block.instructions.iter().rev() {
            if let MirInstructionKind::GcSafePoint { .. } = instruction.kind {
                let roots = live
                    .iter()
                    .copied()
                    .filter(|value| {
                        value_types.get(value).is_some_and(|type_id| {
                            is_managed_reference_type_id(*type_id, Some(arena))
                        })
                    })
                    .collect();
                maps.insert(instruction.result, roots);
            }
            if instruction.has_result() {
                live.remove(&instruction.result);
            }
            if !matches!(instruction.kind, MirInstructionKind::GcSafePoint { .. }) {
                if let Some(target) = instruction_unwind_target(&instruction.kind)
                    && let Some(cleanup_live) = live_in.get(&target)
                {
                    live.extend(cleanup_live.iter().copied());
                }
                live.extend(instruction_operands(&instruction.kind));
            }
        }
    }
    maps
}

fn normal_live_out(
    block: &MirBlock,
    live_in: &BTreeMap<BlockId, BTreeSet<ValueId>>,
    blocks: &BTreeMap<BlockId, &MirBlock>,
) -> BTreeSet<ValueId> {
    match block.terminator() {
        MirTerminator::Branch { target, arguments } => {
            edge_live_values(*target, arguments, live_in, blocks)
        }
        MirTerminator::ConditionalBranch {
            when_true,
            when_false,
            ..
        } => {
            let mut outgoing = edge_live_values(*when_true, &[], live_in, blocks);
            outgoing.extend(edge_live_values(*when_false, &[], live_in, blocks));
            outgoing
        }
        MirTerminator::UnionSwitch { arms, .. } => arms
            .iter()
            .flat_map(|arm| live_in.get(&arm.target).into_iter().flatten().copied())
            .collect(),
        MirTerminator::Missing
        | MirTerminator::Return { .. }
        | MirTerminator::Trap(_)
        | MirTerminator::Panic(_)
        | MirTerminator::ContinueUnwind(_)
        | MirTerminator::Unreachable => BTreeSet::new(),
    }
}

fn edge_live_values(
    target: BlockId,
    arguments: &[ValueId],
    live_in: &BTreeMap<BlockId, BTreeSet<ValueId>>,
    blocks: &BTreeMap<BlockId, &MirBlock>,
) -> BTreeSet<ValueId> {
    let mut live = live_in.get(&target).cloned().unwrap_or_default();
    let Some(target) = blocks.get(&target) else {
        return live;
    };
    for (parameter, argument) in target.arguments().iter().zip(arguments) {
        if live.remove(&parameter.value()) {
            live.insert(*argument);
        }
    }
    live
}

/// Verifies canonical MIR block, value, type, call, and return invariants.
///
/// # Errors
///
/// Returns deterministic invariant violations.
pub fn verify_mir_bubble(
    bubble: &MirBubble,
    arena: &TypeArena,
) -> Result<(), Vec<MirVerificationError>> {
    let signatures: BTreeMap<_, _> = bubble
        .functions
        .iter()
        .map(|function| {
            (
                function.symbol(),
                (
                    function.parameters().to_vec(),
                    function.results().to_vec(),
                    function.effects(),
                ),
            )
        })
        .collect();
    let method_signatures: BTreeMap<_, _> = bubble
        .methods
        .iter()
        .map(|method| {
            (
                method.method,
                (
                    method.function.parameters().to_vec(),
                    method.function.results().to_vec(),
                    method.function.effects(),
                ),
            )
        })
        .collect();
    let mut errors = Vec::new();
    let schema = MirSchema::collect(bubble, arena, &mut errors);
    for function in &bubble.functions {
        verify_function(
            function,
            arena,
            &schema,
            &signatures,
            &method_signatures,
            &mut errors,
        );
    }
    for method in &bubble.methods {
        verify_function(
            &method.function,
            arena,
            &schema,
            &signatures,
            &method_signatures,
            &mut errors,
        );
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[derive(Clone)]
struct DeclaredField {
    owner_types: BTreeSet<TypeId>,
    field_type: TypeId,
    mutable: bool,
}

struct MirSchema<'mir> {
    records: BTreeMap<SymbolId, &'mir MirRecordDeclaration>,
    unions: BTreeMap<SymbolId, &'mir MirUnionDeclaration>,
    classes: BTreeMap<ClassId, &'mir MirClassDeclaration>,
    interfaces: BTreeMap<InterfaceId, &'mir MirInterfaceDeclaration>,
    fields: BTreeMap<FieldId, DeclaredField>,
}

impl<'mir> MirSchema<'mir> {
    fn collect(
        bubble: &'mir MirBubble,
        arena: &TypeArena,
        errors: &mut Vec<MirVerificationError>,
    ) -> Self {
        let mut schema = Self {
            records: BTreeMap::new(),
            unions: BTreeMap::new(),
            classes: BTreeMap::new(),
            interfaces: BTreeMap::new(),
            fields: BTreeMap::new(),
        };
        let mut symbols = BTreeSet::new();
        for declaration in &bubble.declarations {
            if !symbols.insert(declaration.symbol) {
                errors.push(MirVerificationError::DuplicateDeclaration(
                    declaration.symbol,
                ));
            }
            match &declaration.kind {
                MirDeclarationKind::Record(record) => {
                    if !matches!(arena.get(record.type_id), Some(SemanticType::Record(_))) {
                        errors.push(MirVerificationError::InvalidDeclarationType {
                            symbol: declaration.symbol,
                            type_id: record.type_id,
                        });
                    }
                    schema.records.insert(declaration.symbol, record);
                    schema.collect_fields(record.type_id, &record.fields, false, errors);
                }
                MirDeclarationKind::Union(union) => {
                    if arena.get(union.type_id)
                        != Some(&SemanticType::TaggedUnion {
                            definition: declaration.symbol,
                        })
                    {
                        errors.push(MirVerificationError::InvalidDeclarationType {
                            symbol: declaration.symbol,
                            type_id: union.type_id,
                        });
                    }
                    let mut cases = BTreeSet::new();
                    for case in &union.cases {
                        if !cases.insert(case.case) {
                            errors.push(MirVerificationError::DuplicateUnionCase {
                                union: declaration.symbol,
                                case: case.case,
                            });
                        }
                    }
                    schema.unions.insert(declaration.symbol, union);
                }
                MirDeclarationKind::Class(class) => {
                    if arena.get(class.type_id)
                        != Some(&SemanticType::Class {
                            class: class.class,
                            arguments: Vec::new(),
                        })
                    {
                        errors.push(MirVerificationError::InvalidDeclarationType {
                            symbol: declaration.symbol,
                            type_id: class.type_id,
                        });
                    }
                    if schema.classes.insert(class.class, class).is_some() {
                        errors.push(MirVerificationError::DuplicateClass(class.class));
                    }
                    schema.collect_fields(class.type_id, &class.fields, true, errors);
                }
                MirDeclarationKind::Interface(interface) => {
                    if arena.get(interface.type_id)
                        != Some(&SemanticType::Interface {
                            interface: interface.interface,
                            arguments: Vec::new(),
                        })
                    {
                        errors.push(MirVerificationError::InvalidDeclarationType {
                            symbol: declaration.symbol,
                            type_id: interface.type_id,
                        });
                    }
                    schema.interfaces.insert(interface.interface, interface);
                }
            }
        }
        schema
    }

    fn collect_fields(
        &mut self,
        owner_type: TypeId,
        fields: &[MirField],
        mutable: bool,
        errors: &mut Vec<MirVerificationError>,
    ) {
        for field in fields {
            if let Some(existing) = self.fields.get_mut(&field.field) {
                if existing.field_type != field.field_type || existing.mutable != mutable {
                    errors.push(MirVerificationError::DuplicateDeclaredField(field.field));
                } else {
                    existing.owner_types.insert(owner_type);
                }
                continue;
            }
            self.fields.insert(
                field.field,
                DeclaredField {
                    owner_types: BTreeSet::from([owner_type]),
                    field_type: field.field_type,
                    mutable,
                },
            );
        }
    }
}

fn verify_function(
    function: &MirFunction,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    signatures: &BTreeMap<SymbolId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    method_signatures: &BTreeMap<MethodId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    errors: &mut Vec<MirVerificationError>,
) {
    verify_entry_parameters(function, errors);
    let blocks = collect_blocks(function, errors);
    let mut definitions = DefinitionTables::default();
    for block in &function.blocks {
        for argument in &block.arguments {
            definitions.collect(
                argument.value,
                argument.type_id,
                DefinitionSite {
                    block: block.block,
                    instruction: None,
                },
                arena,
                errors,
            );
        }
        for (index, instruction) in block.instructions.iter().enumerate() {
            if let Some(result_type) = instruction.result_type {
                definitions.collect(
                    instruction.result,
                    result_type,
                    DefinitionSite {
                        block: block.block,
                        instruction: Some(index),
                    },
                    arena,
                    errors,
                );
            } else if matches!(instruction.kind, MirInstructionKind::RetainRoot { .. }) {
                definitions.collect_root_handle(
                    instruction.result,
                    DefinitionSite {
                        block: block.block,
                        instruction: Some(index),
                    },
                    errors,
                );
            } else if matches!(instruction.kind, MirInstructionKind::Pin { .. }) {
                definitions.collect_pin_handle(
                    instruction.result,
                    DefinitionSite {
                        block: block.block,
                        instruction: Some(index),
                    },
                    errors,
                );
            } else if !definitions.seen.insert(instruction.result) {
                errors.push(MirVerificationError::DuplicateValue(instruction.result));
            }
        }
    }
    let dominators = compute_dominators(function, &blocks);
    let facts = ControlFlowFacts {
        values: &definitions.values,
        root_handles: &definitions.root_handles,
        pin_handles: &definitions.pin_handles,
        definitions: &definitions.sites,
        dominators: &dominators,
        blocks: &blocks,
    };
    let mut required_function_effects = MirEffectSummary::empty();
    for block in &function.blocks {
        for (index, instruction) in block.instructions.iter().enumerate() {
            for operand in instruction_operands(&instruction.kind) {
                verify_value_use(operand, block.block, index, &facts, errors);
            }
            let referenced_function = match instruction.kind() {
                MirInstructionKind::CallDirect { function, .. }
                | MirInstructionKind::FunctionReference(function) => Some(*function),
                _ => None,
            };
            if let Some(function) = referenced_function
                && !signatures.contains_key(&function)
            {
                errors.push(MirVerificationError::UnknownFunction(function));
            }
            if let MirInstructionKind::CallDirectMethod { method, .. } = instruction.kind()
                && !method_signatures.contains_key(method)
            {
                errors.push(MirVerificationError::UnknownMethod(*method));
            }
            verify_instruction_types(
                instruction,
                arena,
                schema,
                facts.values,
                signatures,
                method_signatures,
                errors,
            );
            let expected_effects =
                expected_instruction_effects(instruction, signatures, method_signatures);
            required_function_effects = required_function_effects.union(expected_effects);
            if instruction.effects() != expected_effects {
                errors.push(MirVerificationError::InstructionEffectMismatch {
                    instruction: instruction.result(),
                    expected: expected_effects,
                    found: instruction.effects(),
                });
            }
            verify_unwind_action(instruction, &blocks, errors);
        }
        required_function_effects =
            required_function_effects.union(terminator_effects(block.terminator()));
        verify_terminator(block, function, arena, schema, &facts, errors);
    }
    if !required_function_effects.is_subset_of(function.effects()) {
        errors.push(MirVerificationError::FunctionEffectMismatch {
            function: function.symbol(),
            expected: required_function_effects,
            found: function.effects(),
        });
    }
    verify_gc_contracts(function, arena, schema, &facts, errors);
}

fn verify_entry_parameters(function: &MirFunction, errors: &mut Vec<MirVerificationError>) {
    let Some(entry) = function.blocks.first() else {
        return;
    };
    if entry.arguments.len() != function.parameters.len() {
        errors.push(MirVerificationError::EntryParameterArity {
            expected: function.parameters.len(),
            found: entry.arguments.len(),
        });
    }
    for (index, (argument, expected)) in
        entry.arguments.iter().zip(&function.parameters).enumerate()
    {
        if argument.type_id != *expected {
            errors.push(MirVerificationError::EntryParameterType {
                index,
                expected: *expected,
                found: argument.type_id,
            });
        }
    }
}

fn expected_instruction_effects(
    instruction: &MirInstruction,
    signatures: &BTreeMap<SymbolId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    method_signatures: &BTreeMap<MethodId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
) -> MirEffectSummary {
    match instruction.kind() {
        MirInstructionKind::CallDirect { function, .. } => signatures
            .get(function)
            .map(|(_, _, effects)| *effects)
            .unwrap_or_default(),
        MirInstructionKind::CallDirectMethod { method, .. } => method_signatures
            .get(method)
            .map(|(_, _, effects)| *effects)
            .unwrap_or_default(),
        MirInstructionKind::CallIndirect {
            declared_effects, ..
        } => *declared_effects,
        kind => local_instruction_effects(kind),
    }
}

fn verify_unwind_action(
    instruction: &MirInstruction,
    blocks: &BTreeMap<BlockId, &MirBlock>,
    errors: &mut Vec<MirVerificationError>,
) {
    let unwind = match instruction.kind() {
        MirInstructionKind::CallDirect { unwind, .. }
        | MirInstructionKind::CallDirectMethod { unwind, .. }
        | MirInstructionKind::CallIndirect { unwind, .. } => *unwind,
        _ => return,
    };
    if let MirUnwindAction::Cleanup(target) = unwind {
        if !instruction.effects().contains(MirEffect::MayUnwind) {
            errors.push(MirVerificationError::InvalidUnwindAction {
                instruction: instruction.result(),
            });
            return;
        }
        let Some(cleanup) = blocks.get(&target) else {
            errors.push(MirVerificationError::InvalidUnwindAction {
                instruction: instruction.result(),
            });
            return;
        };
        if !cleanup.arguments().is_empty() {
            errors.push(MirVerificationError::InvalidUnwindAction {
                instruction: instruction.result(),
            });
        }
    }
}

#[allow(clippy::too_many_lines)]
fn verify_gc_contracts(
    function: &MirFunction,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    facts: &ControlFlowFacts<'_, '_>,
    errors: &mut Vec<MirVerificationError>,
) {
    let expected_roots = expected_safe_point_roots(function, arena);
    for block in &function.blocks {
        let mut straight_line_work = 0_usize;
        for (index, instruction) in block.instructions.iter().enumerate() {
            if straight_line_work >= MAX_STRAIGHT_LINE_WORK_BETWEEN_SAFE_POINTS
                && !matches!(instruction.kind(), MirInstructionKind::GcSafePoint { .. })
            {
                errors.push(MirVerificationError::MissingGcSafePoint {
                    instruction: instruction.result(),
                });
                straight_line_work = 0;
            }
            let requires_safe_point = instruction.effects().contains(MirEffect::Allocates)
                || matches!(
                    instruction.kind(),
                    MirInstructionKind::CallDirect { .. }
                        | MirInstructionKind::CallDirectMethod { .. }
                        | MirInstructionKind::CallIndirect { .. }
                ) && instruction.effects().contains(MirEffect::GcSafePoint);
            if requires_safe_point
                && !index.checked_sub(1).is_some_and(|previous| {
                    matches!(
                        block.instructions()[previous].kind(),
                        MirInstructionKind::GcSafePoint { .. }
                    )
                })
            {
                errors.push(MirVerificationError::MissingGcSafePoint {
                    instruction: instruction.result(),
                });
            }
            match instruction.kind() {
                MirInstructionKind::ArrayMake { element_map, .. } => {
                    if *element_map != array_element_map(arena, instruction.result_type()) {
                        errors.push(MirVerificationError::InvalidObjectMap {
                            instruction: instruction.result(),
                        });
                    }
                }
                MirInstructionKind::TableMake {
                    key_map, value_map, ..
                } => {
                    if (*key_map, *value_map)
                        != table_element_maps(arena, instruction.result_type())
                    {
                        errors.push(MirVerificationError::InvalidObjectMap {
                            instruction: instruction.result(),
                        });
                    }
                }
                MirInstructionKind::ClassMake {
                    class, object_map, ..
                } => {
                    if schema.classes.get(class).is_some_and(|declaration| {
                        expected_class_object_map(declaration, arena) != *object_map
                    }) {
                        errors.push(MirVerificationError::InvalidObjectMap {
                            instruction: instruction.result(),
                        });
                    }
                }
                MirInstructionKind::GcSafePoint {
                    safe_point,
                    roots,
                    stack_map,
                } => {
                    let expected = expected_roots
                        .get(&instruction.result())
                        .cloned()
                        .unwrap_or_default();
                    if roots.len() != expected.len()
                        || stack_map.root_slots().len() != expected.len()
                        || stack_map.safe_point() != *safe_point
                    {
                        errors.push(MirVerificationError::IncompleteStackMap {
                            instruction: instruction.result(),
                            expected: expected.len(),
                            found: roots.len().min(stack_map.root_slots().len()),
                        });
                    }
                    for root in roots {
                        if !expected.contains(root)
                            || !facts.values.get(root).is_some_and(|type_id| {
                                is_managed_reference_type_id(*type_id, Some(arena))
                            })
                        {
                            errors.push(MirVerificationError::InvalidStackMapRoot {
                                instruction: instruction.result(),
                                root: *root,
                            });
                        }
                    }
                    for missing in expected.iter().filter(|root| !roots.contains(root)) {
                        errors.push(MirVerificationError::InvalidStackMapRoot {
                            instruction: instruction.result(),
                            root: *missing,
                        });
                    }
                }
                MirInstructionKind::RetainRoot { value } => {
                    if !facts
                        .values
                        .get(value)
                        .is_some_and(|type_id| is_managed_reference_type_id(*type_id, Some(arena)))
                    {
                        errors.push(MirVerificationError::InvalidStackMapRoot {
                            instruction: instruction.result(),
                            root: *value,
                        });
                    }
                }
                MirInstructionKind::ReleaseRoot { handle } => {
                    if !facts.root_handles.contains(handle) {
                        errors.push(MirVerificationError::ReleaseWithoutRetain {
                            instruction: instruction.result(),
                            value: *handle,
                        });
                    }
                }
                MirInstructionKind::Pin { value } => {
                    if !facts
                        .values
                        .get(value)
                        .is_some_and(|type_id| is_managed_reference_type_id(*type_id, Some(arena)))
                    {
                        errors.push(MirVerificationError::InvalidPinnedReference {
                            instruction: instruction.result(),
                            value: *value,
                        });
                    }
                }
                MirInstructionKind::Unpin { handle } => {
                    if !facts.pin_handles.contains(handle) {
                        errors.push(MirVerificationError::UnpinWithoutPin {
                            instruction: instruction.result(),
                            value: *handle,
                        });
                    }
                }
                MirInstructionKind::WriteBarrier {
                    owner,
                    slot,
                    previous,
                    value,
                } => {
                    verify_write_barrier(
                        instruction,
                        *owner,
                        *slot,
                        *previous,
                        *value,
                        arena,
                        schema,
                        facts.values,
                        errors,
                    );
                    let followed_by_matching_store = block
                        .instructions()
                        .get(index.saturating_add(1))
                        .is_some_and(|next| {
                            matches!(
                                (next.kind(), value),
                                (
                                    MirInstructionKind::FieldSet {
                                        base,
                                        value: stored,
                                        ..
                                    },
                                    Some(barrier_value),
                                ) if base == owner && stored == barrier_value
                            )
                        });
                    if !followed_by_matching_store {
                        errors.push(MirVerificationError::UnexpectedWriteBarrier {
                            instruction: instruction.result(),
                        });
                    }
                }
                MirInstructionKind::FieldSet { base, field, value } => verify_field_store_barrier(
                    instruction,
                    block,
                    index,
                    *base,
                    *field,
                    *value,
                    arena,
                    schema,
                    errors,
                ),
                _ => {}
            }
            if matches!(instruction.kind(), MirInstructionKind::GcSafePoint { .. }) {
                straight_line_work = 0;
            } else {
                straight_line_work = straight_line_work.saturating_add(1);
            }
        }
        let has_backedge = terminator_targets(block.terminator())
            .into_iter()
            .any(|target| target <= block.block());
        if has_backedge
            && !block.instructions().last().is_some_and(|instruction| {
                matches!(instruction.kind(), MirInstructionKind::GcSafePoint { .. })
            })
        {
            errors.push(MirVerificationError::MissingBackedgeSafePoint(
                block.block(),
            ));
        }
    }
    verify_root_balance(function, errors);
    verify_pin_balance(function, errors);
}

fn expected_class_object_map(declaration: &MirClassDeclaration, arena: &TypeArena) -> ObjectMap {
    let references = declaration
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, field)| is_managed_reference_type_id(field.field_type(), Some(arena)))
        .map(|(index, _)| ObjectSlot::new(u32::try_from(index).unwrap_or(u32::MAX)))
        .collect();
    ObjectMap::new(
        u32::try_from(declaration.fields().len()).unwrap_or(u32::MAX),
        references,
    )
    .expect("declared class fields form a canonical object map")
}

#[allow(clippy::too_many_arguments)]
fn verify_write_barrier(
    instruction: &MirInstruction,
    owner: ValueId,
    slot: ObjectSlot,
    previous: Option<ValueId>,
    value: Option<ValueId>,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(owner_type) = values.get(&owner).copied() else {
        return;
    };
    let valid_slot = schema.classes.values().any(|class| {
        class.type_id() == owner_type
            && expected_class_object_map(class, arena).is_reference_slot(slot)
    });
    let operands_are_references = previous.into_iter().chain(value).all(|operand| {
        values
            .get(&operand)
            .is_some_and(|type_id| is_managed_reference_type_id(*type_id, Some(arena)))
    });
    if !valid_slot || !operands_are_references {
        errors.push(MirVerificationError::UnexpectedWriteBarrier {
            instruction: instruction.result(),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_field_store_barrier(
    instruction: &MirInstruction,
    block: &MirBlock,
    index: usize,
    base: ValueId,
    field: FieldId,
    value: ValueId,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(declared) = schema.fields.get(&field) else {
        return;
    };
    if !is_managed_reference_type_id(declared.field_type, Some(arena)) {
        return;
    }
    let expected_slot = schema.classes.values().find_map(|class| {
        class
            .fields()
            .iter()
            .position(|candidate| candidate.field() == field)
            .map(|position| ObjectSlot::new(u32::try_from(position).unwrap_or(u32::MAX)))
    });
    let Some(previous_instruction) = index
        .checked_sub(1)
        .and_then(|previous| block.instructions().get(previous))
    else {
        errors.push(MirVerificationError::MissingWriteBarrier {
            instruction: instruction.result(),
            field,
        });
        return;
    };
    let valid = matches!(
        previous_instruction.kind(),
        MirInstructionKind::WriteBarrier {
            owner,
            slot,
            value: Some(stored),
            ..
        } if *owner == base && Some(*slot) == expected_slot && *stored == value
    );
    if !valid {
        errors.push(MirVerificationError::MissingWriteBarrier {
            instruction: instruction.result(),
            field,
        });
    }
}

fn verify_root_balance(function: &MirFunction, errors: &mut Vec<MirVerificationError>) {
    verify_handle_balance(function, HandleKind::Root, errors);
}

fn verify_pin_balance(function: &MirFunction, errors: &mut Vec<MirVerificationError>) {
    verify_handle_balance(function, HandleKind::Pin, errors);
}

#[derive(Clone, Copy)]
enum HandleKind {
    Root,
    Pin,
}

impl HandleKind {
    const fn acquires(self, instruction: &MirInstructionKind) -> bool {
        matches!(
            (self, instruction),
            (Self::Root, MirInstructionKind::RetainRoot { .. })
                | (Self::Pin, MirInstructionKind::Pin { .. })
        )
    }

    const fn released_handle(self, instruction: &MirInstructionKind) -> Option<ValueId> {
        match (self, instruction) {
            (Self::Root, MirInstructionKind::ReleaseRoot { handle })
            | (Self::Pin, MirInstructionKind::Unpin { handle }) => Some(*handle),
            _ => None,
        }
    }

    const fn release_without_acquire(
        self,
        instruction: ValueId,
        value: ValueId,
    ) -> MirVerificationError {
        match self {
            Self::Root => MirVerificationError::ReleaseWithoutRetain { instruction, value },
            Self::Pin => MirVerificationError::UnpinWithoutPin { instruction, value },
        }
    }

    const fn unreleased(self, block: BlockId, value: ValueId) -> MirVerificationError {
        match self {
            Self::Root => MirVerificationError::UnreleasedRoot { block, value },
            Self::Pin => MirVerificationError::UnreleasedPin { block, value },
        }
    }

    const fn state_mismatch(self, target: BlockId) -> MirVerificationError {
        match self {
            Self::Root => MirVerificationError::RootStateMismatch(target),
            Self::Pin => MirVerificationError::PinStateMismatch(target),
        }
    }
}

fn verify_handle_balance(
    function: &MirFunction,
    kind: HandleKind,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(entry) = function.blocks.first() else {
        return;
    };
    let blocks: BTreeMap<_, _> = function
        .blocks
        .iter()
        .map(|block| (block.block(), block))
        .collect();
    let mut incoming = BTreeMap::<BlockId, BTreeSet<ValueId>>::new();
    incoming.insert(entry.block(), BTreeSet::new());
    let mut pending = vec![entry.block()];
    while let Some(block_id) = pending.pop() {
        let Some(block) = blocks.get(&block_id).copied() else {
            continue;
        };
        let mut retained = incoming.get(&block_id).cloned().unwrap_or_default();
        for instruction in block.instructions() {
            if kind.acquires(instruction.kind()) {
                retained.insert(instruction.result());
            }
            if let Some(handle) = kind.released_handle(instruction.kind())
                && !retained.remove(&handle)
            {
                errors.push(kind.release_without_acquire(instruction.result(), handle));
            }
            let catches_unwind = matches!(
                instruction.kind(),
                MirInstructionKind::CallDirect {
                    unwind: MirUnwindAction::Cleanup(_),
                    ..
                } | MirInstructionKind::CallDirectMethod {
                    unwind: MirUnwindAction::Cleanup(_),
                    ..
                } | MirInstructionKind::CallIndirect {
                    unwind: MirUnwindAction::Cleanup(_),
                    ..
                }
            );
            let propagates_unwind =
                instruction.effects().contains(MirEffect::MayUnwind) && !catches_unwind;
            if instruction.effects().contains(MirEffect::MayTrap) || propagates_unwind {
                for value in &retained {
                    errors.push(kind.unreleased(block_id, *value));
                }
            }
            if let Some(target) = instruction_unwind_target(instruction.kind()) {
                merge_handle_state(target, &retained, &mut incoming, &mut pending, kind, errors);
            }
        }
        let targets = terminator_targets(block.terminator());
        if targets.is_empty() {
            for value in retained {
                errors.push(kind.unreleased(block_id, value));
            }
            continue;
        }
        for target in targets {
            let edge_state = match block.terminator() {
                MirTerminator::Branch { arguments, .. } => {
                    translate_handle_state(target, arguments, &retained, &blocks)
                }
                _ => retained.clone(),
            };
            merge_handle_state(
                target,
                &edge_state,
                &mut incoming,
                &mut pending,
                kind,
                errors,
            );
        }
    }
}

fn translate_handle_state(
    target: BlockId,
    arguments: &[ValueId],
    retained: &BTreeSet<ValueId>,
    blocks: &BTreeMap<BlockId, &MirBlock>,
) -> BTreeSet<ValueId> {
    let mut translated = retained.clone();
    let Some(target) = blocks.get(&target) else {
        return translated;
    };
    for (parameter, argument) in target.arguments().iter().zip(arguments) {
        if translated.remove(argument) {
            translated.insert(parameter.value());
        }
    }
    translated
}

fn merge_handle_state(
    target: BlockId,
    retained: &BTreeSet<ValueId>,
    incoming: &mut BTreeMap<BlockId, BTreeSet<ValueId>>,
    pending: &mut Vec<BlockId>,
    kind: HandleKind,
    errors: &mut Vec<MirVerificationError>,
) {
    match incoming.get(&target) {
        Some(existing) if existing != retained => {
            errors.push(kind.state_mismatch(target));
        }
        Some(_) => {}
        None => {
            incoming.insert(target, retained.clone());
            pending.push(target);
        }
    }
}

#[derive(Clone, Copy)]
struct DefinitionSite {
    block: BlockId,
    instruction: Option<usize>,
}

#[derive(Default)]
struct DefinitionTables {
    values: BTreeMap<ValueId, TypeId>,
    root_handles: BTreeSet<ValueId>,
    pin_handles: BTreeSet<ValueId>,
    sites: BTreeMap<ValueId, DefinitionSite>,
    seen: BTreeSet<ValueId>,
}

impl DefinitionTables {
    fn collect(
        &mut self,
        value: ValueId,
        type_id: TypeId,
        site: DefinitionSite,
        arena: &TypeArena,
        errors: &mut Vec<MirVerificationError>,
    ) {
        if !arena.is_valid_hir_type(type_id) {
            errors.push(MirVerificationError::InvalidType(type_id));
        }
        if !self.seen.insert(value) {
            errors.push(MirVerificationError::DuplicateValue(value));
            return;
        }
        self.values.insert(value, type_id);
        self.sites.insert(value, site);
    }

    fn collect_root_handle(
        &mut self,
        value: ValueId,
        site: DefinitionSite,
        errors: &mut Vec<MirVerificationError>,
    ) {
        if !self.seen.insert(value) {
            errors.push(MirVerificationError::DuplicateValue(value));
            return;
        }
        self.root_handles.insert(value);
        self.sites.insert(value, site);
    }

    fn collect_pin_handle(
        &mut self,
        value: ValueId,
        site: DefinitionSite,
        errors: &mut Vec<MirVerificationError>,
    ) {
        if !self.seen.insert(value) {
            errors.push(MirVerificationError::DuplicateValue(value));
            return;
        }
        self.pin_handles.insert(value);
        self.sites.insert(value, site);
    }
}

struct ControlFlowFacts<'facts, 'function> {
    values: &'facts BTreeMap<ValueId, TypeId>,
    root_handles: &'facts BTreeSet<ValueId>,
    pin_handles: &'facts BTreeSet<ValueId>,
    definitions: &'facts BTreeMap<ValueId, DefinitionSite>,
    dominators: &'facts BTreeMap<BlockId, BTreeSet<BlockId>>,
    blocks: &'facts BTreeMap<BlockId, &'function MirBlock>,
}

fn collect_blocks<'function>(
    function: &'function MirFunction,
    errors: &mut Vec<MirVerificationError>,
) -> BTreeMap<BlockId, &'function MirBlock> {
    let mut blocks = BTreeMap::new();
    for block in &function.blocks {
        if block.block.raw() as usize >= function.blocks.len() {
            errors.push(MirVerificationError::InvalidBlock(block.block));
        }
        if blocks.insert(block.block, block).is_some() {
            errors.push(MirVerificationError::DuplicateBlock(block.block));
        }
    }
    blocks
}

fn compute_dominators(
    function: &MirFunction,
    blocks: &BTreeMap<BlockId, &MirBlock>,
) -> BTreeMap<BlockId, BTreeSet<BlockId>> {
    let Some(entry) = function.blocks.first().map(MirBlock::block) else {
        return BTreeMap::new();
    };
    let mut predecessors: BTreeMap<_, BTreeSet<_>> = blocks
        .keys()
        .map(|block| (*block, BTreeSet::new()))
        .collect();
    for block in &function.blocks {
        for target in block_targets(block) {
            if let Some(target_predecessors) = predecessors.get_mut(&target) {
                target_predecessors.insert(block.block());
            }
        }
    }
    let reachable = reachable_blocks(entry, blocks);
    let mut dominators: BTreeMap<_, _> = blocks
        .keys()
        .map(|block| {
            let initial = if *block == entry || !reachable.contains(block) {
                BTreeSet::from([*block])
            } else {
                reachable.clone()
            };
            (*block, initial)
        })
        .collect();
    loop {
        let mut changed = false;
        for block in reachable.iter().copied().filter(|block| *block != entry) {
            let mut incoming = predecessors[&block]
                .iter()
                .filter(|predecessor| reachable.contains(predecessor))
                .map(|predecessor| dominators[predecessor].clone());
            let mut next = incoming.next().unwrap_or_default();
            for predecessor_dominators in incoming {
                next = next
                    .intersection(&predecessor_dominators)
                    .copied()
                    .collect();
            }
            next.insert(block);
            if dominators[&block] != next {
                dominators.insert(block, next);
                changed = true;
            }
        }
        if !changed {
            return dominators;
        }
    }
}

fn reachable_blocks(entry: BlockId, blocks: &BTreeMap<BlockId, &MirBlock>) -> BTreeSet<BlockId> {
    let mut reachable = BTreeSet::new();
    let mut pending = vec![entry];
    while let Some(block) = pending.pop() {
        if !reachable.insert(block) {
            continue;
        }
        if let Some(block) = blocks.get(&block) {
            pending.extend(
                block_targets(block)
                    .into_iter()
                    .filter(|target| blocks.contains_key(target)),
            );
        }
    }
    reachable
}

fn terminator_targets(terminator: &MirTerminator) -> Vec<BlockId> {
    match terminator {
        MirTerminator::Branch { target, .. } => vec![*target],
        MirTerminator::ConditionalBranch {
            when_true,
            when_false,
            ..
        } => vec![*when_true, *when_false],
        MirTerminator::UnionSwitch { arms, .. } => arms.iter().map(|arm| arm.target).collect(),
        MirTerminator::Missing
        | MirTerminator::Return { .. }
        | MirTerminator::Trap(_)
        | MirTerminator::Panic(_)
        | MirTerminator::ContinueUnwind(_)
        | MirTerminator::Unreachable => Vec::new(),
    }
}

fn terminator_operands(terminator: &MirTerminator) -> Vec<ValueId> {
    match terminator {
        MirTerminator::Return { values } => values.clone(),
        MirTerminator::ConditionalBranch { condition, .. } => vec![*condition],
        MirTerminator::UnionSwitch { scrutinee, .. } => vec![*scrutinee],
        MirTerminator::Missing
        | MirTerminator::Branch { .. }
        | MirTerminator::Trap(_)
        | MirTerminator::Panic(_)
        | MirTerminator::ContinueUnwind(_)
        | MirTerminator::Unreachable => Vec::new(),
    }
}

fn instruction_unwind_target(instruction: &MirInstructionKind) -> Option<BlockId> {
    match instruction {
        MirInstructionKind::CallDirect {
            unwind: MirUnwindAction::Cleanup(target),
            ..
        }
        | MirInstructionKind::CallDirectMethod {
            unwind: MirUnwindAction::Cleanup(target),
            ..
        }
        | MirInstructionKind::CallIndirect {
            unwind: MirUnwindAction::Cleanup(target),
            ..
        } => Some(*target),
        _ => None,
    }
}

fn block_targets(block: &MirBlock) -> Vec<BlockId> {
    let mut targets = terminator_targets(&block.terminator);
    targets.extend(
        block
            .instructions
            .iter()
            .filter_map(|instruction| instruction_unwind_target(&instruction.kind)),
    );
    targets.sort_unstable();
    targets.dedup();
    targets
}

fn verify_instruction_types(
    instruction: &MirInstruction,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    signatures: &BTreeMap<SymbolId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    method_signatures: &BTreeMap<MethodId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    errors: &mut Vec<MirVerificationError>,
) {
    let requires_effect_form = matches!(
        instruction.kind(),
        MirInstructionKind::GcSafePoint { .. }
            | MirInstructionKind::RetainRoot { .. }
            | MirInstructionKind::ReleaseRoot { .. }
            | MirInstructionKind::Pin { .. }
            | MirInstructionKind::Unpin { .. }
            | MirInstructionKind::WriteBarrier { .. }
    );
    if requires_effect_form && instruction.has_result() {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: instruction.result_type(),
        });
        return;
    }
    if verify_numeric_instruction(instruction, arena, values, errors) {
        return;
    }
    if verify_schema_instruction(instruction, arena, schema, values, errors) {
        return;
    }
    if verify_callable_instruction(
        instruction,
        arena,
        values,
        signatures,
        method_signatures,
        errors,
    ) {
        return;
    }
    match instruction.kind() {
        MirInstructionKind::CompareEqual { left, right }
        | MirInstructionKind::CompareNotEqual { left, right } => {
            verify_equality_instruction(instruction, *left, *right, arena, values, errors);
        }
        MirInstructionKind::ArrayMake { elements, .. } => {
            let Some(SemanticType::Array(element_type)) =
                arena.get(instruction.result_type()).cloned()
            else {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
                return;
            };
            for operand in elements {
                verify_operand_type(instruction.result(), *operand, element_type, values, errors);
            }
        }
        MirInstructionKind::ArrayCreate {
            length,
            initial_value,
            element_map,
        } => {
            let Some(SemanticType::Array(element_type)) =
                arena.get(instruction.result_type()).cloned()
            else {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
                return;
            };
            if let Some(integer) = arena.source_type("Int") {
                verify_operand_type(instruction.result(), *length, integer, values, errors);
            }
            verify_operand_type(
                instruction.result(),
                *initial_value,
                element_type,
                values,
                errors,
            );
            if *element_map != array_element_map(arena, instruction.result_type()) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::TableMake { entries, .. } => {
            let Some(SemanticType::Table { key, value }) =
                arena.get(instruction.result_type()).cloned()
            else {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
                return;
            };
            for (entry_key, entry_value) in entries {
                verify_operand_type(instruction.result(), *entry_key, key, values, errors);
                verify_operand_type(instruction.result(), *entry_value, value, values, errors);
            }
        }
        MirInstructionKind::ArrayGet { array, index } => {
            let Some(array_type) = values.get(array).copied() else {
                return;
            };
            let Some(SemanticType::Array(element_type)) = arena.get(array_type).cloned() else {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *array,
                    found: array_type,
                });
                return;
            };
            if let Some(integer) = arena.source_type("Int") {
                verify_operand_type(instruction.result(), *index, integer, values, errors);
            }
            if !is_optional_of(arena, instruction.result_type(), element_type) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ArrayLength { array } => {
            let Some(array_type) = values.get(array).copied() else {
                return;
            };
            if !matches!(arena.get(array_type), Some(SemanticType::Array(_))) {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *array,
                    found: array_type,
                });
            }
            if arena.source_type("Int") != Some(instruction.result_type()) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ArrayGetChecked { array, index } => {
            let Some(array_type) = values.get(array).copied() else {
                return;
            };
            let Some(SemanticType::Array(element_type)) = arena.get(array_type).cloned() else {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *array,
                    found: array_type,
                });
                return;
            };
            if let Some(integer) = arena.source_type("Int") {
                verify_operand_type(instruction.result(), *index, integer, values, errors);
            }
            if instruction.result_type() != element_type {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ArraySet {
            array,
            index,
            value,
            element_map,
        } => {
            let Some(array_type) = values.get(array).copied() else {
                return;
            };
            let Some(SemanticType::Array(element_type)) = arena.get(array_type).cloned() else {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *array,
                    found: array_type,
                });
                return;
            };
            if let Some(integer) = arena.source_type("Int") {
                verify_operand_type(instruction.result(), *index, integer, values, errors);
            }
            verify_operand_type(instruction.result(), *value, element_type, values, errors);
            if *element_map != array_element_map(arena, array_type) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
            if arena.source_type("nil") != Some(instruction.result_type()) {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::ArrayFill {
            array,
            value,
            element_map,
        } => {
            let Some(array_type) = values.get(array).copied() else {
                return;
            };
            let Some(SemanticType::Array(element_type)) = arena.get(array_type).cloned() else {
                errors.push(MirVerificationError::InvalidCollectionOperand {
                    instruction: instruction.result(),
                    operand: *array,
                    found: array_type,
                });
                return;
            };
            verify_operand_type(instruction.result(), *value, element_type, values, errors);
            if *element_map != array_element_map(arena, array_type)
                || arena.source_type("nil") != Some(instruction.result_type())
            {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        _ => {}
    }
}

fn verify_numeric_instruction(
    instruction: &MirInstruction,
    arena: &TypeArena,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) -> bool {
    match instruction.kind() {
        MirInstructionKind::IntegerConstant(value) => {
            verify_numeric_result(instruction, integer_type(arena, value.kind()), errors);
        }
        MirInstructionKind::FloatConstant(value) => {
            verify_numeric_result(instruction, float_type(arena, value.kind()), errors);
        }
        MirInstructionKind::CheckedIntegerAdd { kind, left, right }
        | MirInstructionKind::CheckedIntegerSubtract { kind, left, right }
        | MirInstructionKind::CheckedIntegerMultiply { kind, left, right }
        | MirInstructionKind::CheckedIntegerDivide { kind, left, right }
        | MirInstructionKind::CheckedIntegerRemainder { kind, left, right } => {
            verify_numeric_binary(
                instruction,
                (*left, *right),
                integer_type(arena, *kind),
                false,
                arena,
                values,
                errors,
            );
        }
        MirInstructionKind::FloatAdd { kind, left, right }
        | MirInstructionKind::FloatSubtract { kind, left, right }
        | MirInstructionKind::FloatMultiply { kind, left, right }
        | MirInstructionKind::FloatDivide { kind, left, right } => {
            verify_numeric_binary(
                instruction,
                (*left, *right),
                float_type(arena, *kind),
                false,
                arena,
                values,
                errors,
            );
        }
        MirInstructionKind::IntegerNegate { kind, operand } => {
            verify_numeric_unary(
                instruction,
                *operand,
                integer_type(arena, *kind),
                values,
                errors,
            );
        }
        MirInstructionKind::FloatNegate { kind, operand } => {
            verify_numeric_unary(
                instruction,
                *operand,
                float_type(arena, *kind),
                values,
                errors,
            );
        }
        MirInstructionKind::CompareIntegerLess { kind, left, right }
        | MirInstructionKind::CompareIntegerGreater { kind, left, right } => {
            verify_numeric_binary(
                instruction,
                (*left, *right),
                integer_type(arena, *kind),
                true,
                arena,
                values,
                errors,
            );
        }
        MirInstructionKind::CompareFloatLess { kind, left, right }
        | MirInstructionKind::CompareFloatGreater { kind, left, right } => {
            verify_numeric_binary(
                instruction,
                (*left, *right),
                float_type(arena, *kind),
                true,
                arena,
                values,
                errors,
            );
        }
        _ => return false,
    }
    true
}

fn verify_numeric_binary(
    instruction: &MirInstruction,
    operands: (ValueId, ValueId),
    operand_type: Option<TypeId>,
    comparison: bool,
    arena: &TypeArena,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(operand_type) = operand_type else {
        if let Some(result_type) = instruction.optional_result_type() {
            errors.push(MirVerificationError::InvalidInstructionType {
                instruction: instruction.result(),
                result_type,
            });
        }
        return;
    };
    verify_operand_type(
        instruction.result(),
        operands.0,
        operand_type,
        values,
        errors,
    );
    verify_operand_type(
        instruction.result(),
        operands.1,
        operand_type,
        values,
        errors,
    );
    let expected_result = if comparison {
        arena.source_type("Boolean")
    } else {
        Some(operand_type)
    };
    verify_numeric_result(instruction, expected_result, errors);
}

fn verify_numeric_unary(
    instruction: &MirInstruction,
    operand: ValueId,
    operand_type: Option<TypeId>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(operand_type) = operand_type else {
        if let Some(result_type) = instruction.optional_result_type() {
            errors.push(MirVerificationError::InvalidInstructionType {
                instruction: instruction.result(),
                result_type,
            });
        }
        return;
    };
    verify_operand_type(instruction.result(), operand, operand_type, values, errors);
    verify_numeric_result(instruction, Some(operand_type), errors);
}

fn verify_numeric_result(
    instruction: &MirInstruction,
    expected: Option<TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    if let (Some(found), Some(expected)) = (instruction.optional_result_type(), expected)
        && found != expected
    {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: found,
        });
    }
}

fn integer_type(arena: &TypeArena, kind: IntegerKind) -> Option<TypeId> {
    arena.source_type(integer_kind_text(kind))
}

fn float_type(arena: &TypeArena, kind: FloatKind) -> Option<TypeId> {
    arena.source_type(float_kind_text(kind))
}

fn verify_equality_instruction(
    instruction: &MirInstruction,
    left: ValueId,
    right: ValueId,
    arena: &TypeArena,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some((left_type, right_type)) = values.get(&left).copied().zip(values.get(&right).copied())
    else {
        return;
    };
    if arena.source_type("Boolean") != Some(instruction.result_type()) {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: instruction.result_type(),
        });
    }
    if !mir_equality_comparable(arena, left_type, right_type) {
        errors.push(MirVerificationError::InvalidComparisonOperands {
            instruction: instruction.result(),
            left: left_type,
            right: right_type,
        });
    }
}

fn mir_equality_comparable(arena: &TypeArena, left: TypeId, right: TypeId) -> bool {
    left == right && mir_supports_default_equality(arena, left)
}

fn mir_supports_default_equality(arena: &TypeArena, type_id: TypeId) -> bool {
    match arena.get(type_id) {
        Some(
            SemanticType::Primitive(
                pop_types::PrimitiveType::Nil
                | pop_types::PrimitiveType::Boolean
                | pop_types::PrimitiveType::Integer(_)
                | pop_types::PrimitiveType::String,
            )
            | SemanticType::Class { .. },
        ) => true,
        Some(SemanticType::Tuple(elements) | SemanticType::Union(elements)) => elements
            .iter()
            .all(|element| mir_supports_default_equality(arena, *element)),
        Some(SemanticType::Record(fields)) => fields
            .iter()
            .all(|(_, field_type)| mir_supports_default_equality(arena, *field_type)),
        _ => false,
    }
}

fn verify_schema_instruction(
    instruction: &MirInstruction,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) -> bool {
    match instruction.kind() {
        MirInstructionKind::RecordMake { record, fields } => {
            let Some(declaration) = schema.records.get(record) else {
                errors.push(MirVerificationError::UnknownRecord {
                    instruction: instruction.result(),
                    record: *record,
                });
                return true;
            };
            verify_aggregate_result(instruction, declaration.type_id, errors);
            verify_constructed_fields(
                instruction,
                fields,
                &declaration.fields,
                true,
                values,
                errors,
            );
        }
        MirInstructionKind::ClassMake { class, fields, .. } => {
            let Some(declaration) = schema.classes.get(class) else {
                errors.push(MirVerificationError::UnknownClass {
                    instruction: instruction.result(),
                    class: *class,
                });
                return true;
            };
            verify_aggregate_result(instruction, declaration.type_id, errors);
            verify_constructed_fields(
                instruction,
                fields,
                &declaration.fields,
                true,
                values,
                errors,
            );
        }
        MirInstructionKind::RecordUpdate {
            record,
            base,
            fields,
        } => {
            let Some(declaration) = schema.records.get(record) else {
                errors.push(MirVerificationError::UnknownRecord {
                    instruction: instruction.result(),
                    record: *record,
                });
                return true;
            };
            verify_aggregate_result(instruction, declaration.type_id, errors);
            verify_operand_type(
                instruction.result(),
                *base,
                declaration.type_id,
                values,
                errors,
            );
            verify_constructed_fields(
                instruction,
                fields,
                &declaration.fields,
                false,
                values,
                errors,
            );
        }
        MirInstructionKind::FieldGet { base, field } => {
            verify_field_get(instruction, *base, *field, schema, values, errors);
        }
        MirInstructionKind::FieldSet { base, field, value } => {
            verify_field_set(
                instruction,
                FieldSetOperands {
                    base: *base,
                    field: *field,
                    value: *value,
                },
                arena,
                schema,
                values,
                errors,
            );
        }
        MirInstructionKind::UnionMake {
            union,
            case,
            arguments,
        } => verify_union_make(
            instruction,
            *union,
            *case,
            arguments,
            schema,
            values,
            errors,
        ),
        _ => return false,
    }
    true
}

#[derive(Clone, Copy)]
struct FieldSetOperands {
    base: ValueId,
    field: FieldId,
    value: ValueId,
}

fn verify_aggregate_result(
    instruction: &MirInstruction,
    expected: TypeId,
    errors: &mut Vec<MirVerificationError>,
) {
    if instruction.result_type() != expected {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: instruction.result_type(),
        });
    }
}

fn verify_constructed_fields(
    instruction: &MirInstruction,
    fields: &[(FieldId, ValueId)],
    declared: &[MirField],
    require_complete: bool,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let mut seen = BTreeSet::new();
    for (field, value) in fields {
        if !seen.insert(*field) {
            errors.push(MirVerificationError::DuplicateDeclaredField(*field));
        }
        let Some(declared) = declared.iter().find(|candidate| candidate.field == *field) else {
            errors.push(MirVerificationError::UnknownField {
                instruction: instruction.result(),
                field: *field,
            });
            continue;
        };
        verify_operand_type(
            instruction.result(),
            *value,
            declared.field_type,
            values,
            errors,
        );
    }
    if require_complete {
        for field in declared {
            if !seen.contains(&field.field) {
                errors.push(MirVerificationError::MissingDeclaredField {
                    instruction: instruction.result(),
                    field: field.field,
                });
            }
        }
    }
}

fn verify_field_get(
    instruction: &MirInstruction,
    base: ValueId,
    field: FieldId,
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(declared) = schema.fields.get(&field) else {
        errors.push(MirVerificationError::UnknownField {
            instruction: instruction.result(),
            field,
        });
        return;
    };
    verify_field_owner(instruction, base, field, declared, values, errors);
    verify_aggregate_result(instruction, declared.field_type, errors);
}

fn verify_field_set(
    instruction: &MirInstruction,
    operands: FieldSetOperands,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(declared) = schema.fields.get(&operands.field) else {
        errors.push(MirVerificationError::UnknownField {
            instruction: instruction.result(),
            field: operands.field,
        });
        return;
    };
    verify_field_owner(
        instruction,
        operands.base,
        operands.field,
        declared,
        values,
        errors,
    );
    if !declared.mutable {
        errors.push(MirVerificationError::ImmutableFieldSet {
            instruction: instruction.result(),
            field: operands.field,
        });
    }
    verify_operand_type(
        instruction.result(),
        operands.value,
        declared.field_type,
        values,
        errors,
    );
    if arena.source_type("nil") != Some(instruction.result_type()) {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: instruction.result_type(),
        });
    }
}

fn verify_field_owner(
    instruction: &MirInstruction,
    base: ValueId,
    field: FieldId,
    declared: &DeclaredField,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    if let Some(found) = values.get(&base)
        && !declared.owner_types.contains(found)
    {
        errors.push(MirVerificationError::WrongFieldOwner {
            instruction: instruction.result(),
            field,
            expected: declared
                .owner_types
                .iter()
                .next()
                .copied()
                .unwrap_or(*found),
            found: *found,
        });
    }
}

fn verify_union_make(
    instruction: &MirInstruction,
    union: SymbolId,
    case: UnionCaseId,
    arguments: &[ValueId],
    schema: &MirSchema<'_>,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(declaration) = schema.unions.get(&union) else {
        errors.push(MirVerificationError::UnknownUnion {
            instruction: instruction.result(),
            union,
        });
        return;
    };
    verify_aggregate_result(instruction, declaration.type_id, errors);
    let Some(case_definition) = declaration
        .cases
        .iter()
        .find(|candidate| candidate.case == case)
    else {
        errors.push(MirVerificationError::UnknownUnionCase {
            instruction: instruction.result(),
            union,
            case,
        });
        return;
    };
    for (argument, expected) in arguments.iter().zip(&case_definition.parameters) {
        verify_operand_type(instruction.result(), *argument, *expected, values, errors);
    }
    if arguments.len() != case_definition.parameters.len() {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: instruction.result_type(),
        });
    }
}

fn verify_callable_instruction(
    instruction: &MirInstruction,
    arena: &TypeArena,
    values: &BTreeMap<ValueId, TypeId>,
    signatures: &BTreeMap<SymbolId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    method_signatures: &BTreeMap<MethodId, (Vec<TypeId>, Vec<TypeId>, MirEffectSummary)>,
    errors: &mut Vec<MirVerificationError>,
) -> bool {
    match instruction.kind() {
        MirInstructionKind::FunctionReference(function) => {
            if let Some((parameters, results, _)) = signatures.get(function)
                && arena.get(instruction.result_type())
                    != Some(&SemanticType::Function {
                        parameters: parameters.clone(),
                        results: results.clone(),
                        effects: pop_types::EffectSummary::empty(),
                    })
            {
                errors.push(MirVerificationError::InvalidInstructionType {
                    instruction: instruction.result(),
                    result_type: instruction.result_type(),
                });
            }
        }
        MirInstructionKind::CallDirect {
            function,
            arguments,
            ..
        } => {
            if let Some((parameters, results, _)) = signatures.get(function) {
                verify_call_signature(instruction, arguments, parameters, results, values, errors);
            }
        }
        MirInstructionKind::CallStandard {
            function,
            arguments,
            ..
        } => {
            let parameter = match function.raw() {
                0 => arena.source_type("Int"),
                1 => arena.source_type("String"),
                _ => {
                    errors.push(MirVerificationError::UnknownStandardFunction(*function));
                    None
                }
            };
            if let Some(parameter) = parameter {
                verify_call_signature(instruction, arguments, &[parameter], &[], values, errors);
            }
        }
        MirInstructionKind::CallDirectMethod {
            method, arguments, ..
        } => {
            if let Some((parameters, results, _)) = method_signatures.get(method) {
                verify_call_signature(instruction, arguments, parameters, results, values, errors);
            }
        }
        MirInstructionKind::CallIndirect {
            callee, arguments, ..
        } => {
            verify_indirect_call(instruction, *callee, arguments, arena, values, errors);
        }
        _ => return false,
    }
    true
}

fn verify_indirect_call(
    instruction: &MirInstruction,
    callee: ValueId,
    arguments: &[ValueId],
    arena: &TypeArena,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(callee_type) = values.get(&callee).copied() else {
        return;
    };
    let Some(SemanticType::Function {
        parameters,
        results,
        ..
    }) = arena.get(callee_type).cloned()
    else {
        errors.push(MirVerificationError::InvalidCallableOperand {
            instruction: instruction.result(),
            operand: callee,
            found: callee_type,
        });
        return;
    };
    verify_call_signature(
        instruction,
        arguments,
        &parameters,
        &results,
        values,
        errors,
    );
}

fn verify_call_signature(
    instruction: &MirInstruction,
    arguments: &[ValueId],
    parameters: &[TypeId],
    results: &[TypeId],
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    for (argument, expected) in arguments.iter().zip(parameters) {
        verify_operand_type(instruction.result(), *argument, *expected, values, errors);
    }
    let found_results = usize::from(instruction.has_result());
    if arguments.len() != parameters.len() || results.len() != found_results {
        errors.push(MirVerificationError::InvalidCallSignature {
            instruction: instruction.result(),
            expected_arguments: parameters.len(),
            found_arguments: arguments.len(),
            expected_results: results.len(),
            found_results,
        });
        return;
    }
    if let ([expected], Some(found)) = (results, instruction.optional_result_type())
        && *expected != found
    {
        errors.push(MirVerificationError::InvalidInstructionType {
            instruction: instruction.result(),
            result_type: found,
        });
    }
}

fn is_optional_of(arena: &TypeArena, candidate: TypeId, element: TypeId) -> bool {
    let Some(nil) = arena.source_type("nil") else {
        return false;
    };
    if element == nil {
        return candidate == nil;
    }
    matches!(
        arena.get(candidate),
        Some(SemanticType::Union(members))
            if members.len() == 2 && members.contains(&element) && members.contains(&nil)
    )
}

fn verify_operand_type(
    instruction: ValueId,
    operand: ValueId,
    expected: TypeId,
    values: &BTreeMap<ValueId, TypeId>,
    errors: &mut Vec<MirVerificationError>,
) {
    if let Some(found) = values.get(&operand)
        && *found != expected
    {
        errors.push(MirVerificationError::WrongOperandType {
            instruction,
            operand,
            expected,
            found: *found,
        });
    }
}

fn verify_value_use(
    operand: ValueId,
    use_block: BlockId,
    use_instruction: usize,
    facts: &ControlFlowFacts<'_, '_>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(definition) = facts.definitions.get(&operand).copied() else {
        errors.push(MirVerificationError::UnknownValue(operand));
        return;
    };
    if definition.block == use_block {
        if definition
            .instruction
            .is_some_and(|definition| definition >= use_instruction)
        {
            errors.push(MirVerificationError::ValueUsedBeforeDefinition(operand));
        }
        return;
    }
    if !facts
        .dominators
        .get(&use_block)
        .is_some_and(|blocks| blocks.contains(&definition.block))
    {
        errors.push(MirVerificationError::ValueNotDominated {
            value: operand,
            definition: definition.block,
            use_block,
        });
    }
}

fn verify_terminator(
    block: &MirBlock,
    function: &MirFunction,
    arena: &TypeArena,
    schema: &MirSchema<'_>,
    facts: &ControlFlowFacts<'_, '_>,
    errors: &mut Vec<MirVerificationError>,
) {
    let use_instruction = block.instructions.len();
    match &block.terminator {
        MirTerminator::Missing => errors.push(MirVerificationError::MissingTerminator(block.block)),
        MirTerminator::Branch { target, arguments } => {
            verify_target(*target, facts.blocks, errors);
            for argument in arguments {
                verify_value_use(*argument, block.block, use_instruction, facts, errors);
            }
            verify_edge_arguments(
                block.block,
                *target,
                arguments,
                facts.values,
                facts.blocks,
                errors,
            );
        }
        MirTerminator::ConditionalBranch {
            condition,
            when_true,
            when_false,
        } => {
            verify_value_use(*condition, block.block, use_instruction, facts, errors);
            if let Some(found) = facts.values.get(condition)
                && arena.source_type("Boolean") != Some(*found)
            {
                errors.push(MirVerificationError::ConditionalBranchConditionType {
                    block: block.block,
                    found: *found,
                });
            }
            for target in [*when_true, *when_false] {
                verify_target(target, facts.blocks, errors);
                verify_edge_arguments(block.block, target, &[], facts.values, facts.blocks, errors);
            }
        }
        MirTerminator::UnionSwitch {
            scrutinee,
            union,
            arms,
        } => {
            verify_value_use(*scrutinee, block.block, use_instruction, facts, errors);
            let Some(declaration) = schema.unions.get(union) else {
                errors.push(MirVerificationError::InvalidUnionSwitch { union: *union });
                return;
            };
            if facts.values.get(scrutinee) != Some(&declaration.type_id()) {
                errors.push(MirVerificationError::InvalidUnionSwitch { union: *union });
            }
            let expected: BTreeSet<_> =
                declaration.cases().iter().map(MirUnionCase::case).collect();
            let found: BTreeSet<_> = arms.iter().map(|arm| arm.case).collect();
            if expected != found || found.len() != arms.len() {
                errors.push(MirVerificationError::InvalidUnionSwitch { union: *union });
            }
            for arm in arms {
                verify_target(arm.target, facts.blocks, errors);
                let Some(case) = declaration
                    .cases()
                    .iter()
                    .find(|case| case.case == arm.case)
                else {
                    continue;
                };
                let Some(target) = facts.blocks.get(&arm.target) else {
                    continue;
                };
                if target.arguments.len() != case.parameters.len()
                    || target
                        .arguments
                        .iter()
                        .map(|argument| argument.type_id)
                        .ne(case.parameters.iter().copied())
                {
                    errors.push(MirVerificationError::InvalidUnionSwitch { union: *union });
                }
            }
        }
        MirTerminator::Return { values: returned } => {
            if returned.len() != function.results.len() {
                errors.push(MirVerificationError::WrongReturnArity {
                    expected: function.results.len(),
                    found: returned.len(),
                });
            }
            for (value, expected) in returned.iter().zip(&function.results) {
                verify_value_use(*value, block.block, use_instruction, facts, errors);
                match facts.values.get(value) {
                    Some(found) if found != expected => {
                        errors.push(MirVerificationError::WrongReturnType {
                            expected: *expected,
                            found: *found,
                        });
                    }
                    None => errors.push(MirVerificationError::UnknownValue(*value)),
                    _ => {}
                }
            }
        }
        MirTerminator::Trap(_)
        | MirTerminator::Panic(_)
        | MirTerminator::ContinueUnwind(_)
        | MirTerminator::Unreachable => {}
    }
}

fn verify_target(
    target: BlockId,
    blocks: &BTreeMap<BlockId, &MirBlock>,
    errors: &mut Vec<MirVerificationError>,
) {
    if !blocks.contains_key(&target) {
        errors.push(MirVerificationError::InvalidBlock(target));
    }
}

fn verify_edge_arguments(
    block: BlockId,
    target: BlockId,
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    blocks: &BTreeMap<BlockId, &MirBlock>,
    errors: &mut Vec<MirVerificationError>,
) {
    let Some(target_block) = blocks.get(&target) else {
        return;
    };
    if arguments.len() != target_block.arguments.len() {
        errors.push(MirVerificationError::EdgeArgumentArity {
            block,
            target,
            expected: target_block.arguments.len(),
            found: arguments.len(),
        });
    }
    for (index, (argument, parameter)) in arguments.iter().zip(&target_block.arguments).enumerate()
    {
        if let Some(found) = values.get(argument)
            && *found != parameter.type_id
        {
            errors.push(MirVerificationError::EdgeArgumentType {
                block,
                target,
                index,
                expected: parameter.type_id,
                found: *found,
            });
        }
    }
}

fn instruction_operands(kind: &MirInstructionKind) -> Vec<ValueId> {
    match kind {
        MirInstructionKind::IntegerConstant(_)
        | MirInstructionKind::FloatConstant(_)
        | MirInstructionKind::StringConstant(_)
        | MirInstructionKind::BooleanConstant(_)
        | MirInstructionKind::NilConstant
        | MirInstructionKind::FunctionReference(_)
        | MirInstructionKind::GcSafePoint { .. } => Vec::new(),
        MirInstructionKind::TupleMake(values)
        | MirInstructionKind::ArrayMake {
            elements: values, ..
        }
        | MirInstructionKind::CallDirect {
            arguments: values, ..
        }
        | MirInstructionKind::CallStandard {
            arguments: values, ..
        }
        | MirInstructionKind::CallDirectMethod {
            arguments: values, ..
        }
        | MirInstructionKind::CallInterface {
            arguments: values, ..
        }
        | MirInstructionKind::UnionMake {
            arguments: values, ..
        } => values.clone(),
        MirInstructionKind::ArrayCreate {
            length,
            initial_value,
            ..
        } => vec![*length, *initial_value],
        MirInstructionKind::CallIndirect {
            callee, arguments, ..
        } => std::iter::once(*callee)
            .chain(arguments.iter().copied())
            .collect(),
        MirInstructionKind::CheckedIntegerAdd { left, right, .. }
        | MirInstructionKind::CheckedIntegerSubtract { left, right, .. }
        | MirInstructionKind::CheckedIntegerMultiply { left, right, .. }
        | MirInstructionKind::CheckedIntegerDivide { left, right, .. }
        | MirInstructionKind::CheckedIntegerRemainder { left, right, .. }
        | MirInstructionKind::FloatAdd { left, right, .. }
        | MirInstructionKind::FloatSubtract { left, right, .. }
        | MirInstructionKind::FloatMultiply { left, right, .. }
        | MirInstructionKind::FloatDivide { left, right, .. }
        | MirInstructionKind::BooleanAnd { left, right }
        | MirInstructionKind::BooleanOr { left, right }
        | MirInstructionKind::CompareEqual { left, right }
        | MirInstructionKind::CompareNotEqual { left, right }
        | MirInstructionKind::CompareIntegerLess { left, right, .. }
        | MirInstructionKind::CompareIntegerGreater { left, right, .. }
        | MirInstructionKind::CompareFloatLess { left, right, .. }
        | MirInstructionKind::CompareFloatGreater { left, right, .. } => vec![*left, *right],
        MirInstructionKind::BooleanNot { operand }
        | MirInstructionKind::IntegerNegate { operand, .. }
        | MirInstructionKind::FloatNegate { operand, .. } => vec![*operand],
        MirInstructionKind::ArrayGet { array, index } => vec![*array, *index],
        MirInstructionKind::ArrayLength { array } => vec![*array],
        MirInstructionKind::ArrayGetChecked { array, index } => vec![*array, *index],
        MirInstructionKind::ArraySet {
            array,
            index,
            value,
            ..
        } => vec![*array, *index, *value],
        MirInstructionKind::ArrayFill { array, value, .. } => vec![*array, *value],
        MirInstructionKind::RecordMake { fields, .. } => {
            fields.iter().map(|(_, value)| *value).collect()
        }
        MirInstructionKind::ClassMake { fields, .. } => {
            fields.iter().map(|(_, value)| *value).collect()
        }
        MirInstructionKind::TableMake { entries, .. } => entries
            .iter()
            .flat_map(|(key, value)| [*key, *value])
            .collect(),
        MirInstructionKind::RecordUpdate { base, fields, .. } => std::iter::once(*base)
            .chain(fields.iter().map(|(_, value)| *value))
            .collect(),
        MirInstructionKind::FieldGet { base, .. } => vec![*base],
        MirInstructionKind::InterfaceUpcast { value: base, .. }
        | MirInstructionKind::CaptureCellLoad { cell: base } => vec![*base],
        MirInstructionKind::CaptureCellAllocate { initial, .. } => vec![*initial],
        MirInstructionKind::CaptureCellStore { cell, value } => vec![*cell, *value],
        MirInstructionKind::ClosureEnvironmentAllocate { captures, .. } => captures
            .iter()
            .filter(|capture| !capture.self_reference)
            .map(|capture| capture.value)
            .collect(),
        MirInstructionKind::CaptureStore { value, .. } => vec![*value],
        MirInstructionKind::CaptureLoad { .. }
        | MirInstructionKind::CaptureCellReference { .. } => Vec::new(),
        MirInstructionKind::FieldSet { base, value, .. } => vec![*base, *value],
        MirInstructionKind::RetainRoot { value } => vec![*value],
        MirInstructionKind::ReleaseRoot { handle } => vec![*handle],
        MirInstructionKind::Pin { value } => vec![*value],
        MirInstructionKind::Unpin { handle } => vec![*handle],
        MirInstructionKind::WriteBarrier {
            owner,
            previous,
            value,
            ..
        } => std::iter::once(*owner)
            .chain(previous.iter().copied())
            .chain(value.iter().copied())
            .collect(),
    }
}

fn dump_declaration(output: &mut String, declaration: &MirDeclaration) {
    match &declaration.kind {
        MirDeclarationKind::Record(record) => {
            let _ = write!(
                output,
                "type.record s{} t{} fields ",
                declaration.symbol.raw(),
                record.type_id.raw()
            );
            dump_declared_fields(output, &record.fields);
        }
        MirDeclarationKind::Union(union) => {
            let _ = write!(
                output,
                "type.union s{} t{} cases ",
                declaration.symbol.raw(),
                union.type_id.raw()
            );
            dump_union_cases(output, &union.cases);
        }
        MirDeclarationKind::Class(class) => {
            let _ = write!(
                output,
                "type.class s{} c{} t{} fields ",
                declaration.symbol.raw(),
                class.class.raw(),
                class.type_id.raw()
            );
            dump_declared_fields(output, &class.fields);
            output.push_str(" methods ");
            dump_method_ids(output, &class.methods);
            output.push_str(" implements ");
            dump_interface_implementations(output, &class.interfaces);
        }
        MirDeclarationKind::Interface(interface) => {
            let _ = write!(
                output,
                "type.interface s{} i{} t{} methods ",
                declaration.symbol.raw(),
                interface.interface.raw(),
                interface.type_id.raw()
            );
            dump_interface_methods(output, &interface.methods);
        }
    }
    output.push('\n');
}

fn dump_interface_methods(output: &mut String, methods: &[MirInterfaceMethod]) {
    if methods.is_empty() {
        output.push('-');
        return;
    }
    for (index, method) in methods.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        let _ = write!(output, "im{}@{}(", method.method.raw(), method.slot);
        dump_type_ids(output, &method.parameters);
        output.push_str(")->(");
        dump_type_ids(output, &method.results);
        output.push(')');
    }
}

fn dump_interface_implementations(
    output: &mut String,
    implementations: &[MirInterfaceImplementation],
) {
    if implementations.is_empty() {
        output.push('-');
        return;
    }
    for (index, implementation) in implementations.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        let _ = write!(
            output,
            "i{}:t{}[",
            implementation.interface.raw(),
            implementation.interface_type.raw()
        );
        for (method_index, method) in implementation.methods.iter().enumerate() {
            if method_index > 0 {
                output.push(';');
            }
            let _ = write!(
                output,
                "im{}@{}=m{}",
                method.interface_method.raw(),
                method.slot,
                method.class_method.raw()
            );
        }
        output.push(']');
    }
}

fn dump_type_ids(output: &mut String, types: &[TypeId]) {
    for (index, type_id) in types.iter().enumerate() {
        if index > 0 {
            output.push(';');
        }
        let _ = write!(output, "t{}", type_id.raw());
    }
}

fn dump_declared_fields(output: &mut String, fields: &[MirField]) {
    if fields.is_empty() {
        output.push('-');
        return;
    }
    for (index, field) in fields.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        let _ = write!(
            output,
            "field#{}:t{}",
            field.field.raw(),
            field.field_type.raw()
        );
    }
}

fn dump_union_cases(output: &mut String, cases: &[MirUnionCase]) {
    if cases.is_empty() {
        output.push('-');
        return;
    }
    for (index, case) in cases.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        let _ = write!(output, "case#{}(", case.case.raw());
        for (parameter_index, parameter) in case.parameters.iter().enumerate() {
            if parameter_index != 0 {
                output.push(';');
            }
            let _ = write!(output, "t{}", parameter.raw());
        }
        output.push(')');
    }
}

fn dump_method_ids(output: &mut String, methods: &[MethodId]) {
    if methods.is_empty() {
        output.push('-');
        return;
    }
    for (index, method) in methods.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        let _ = write!(output, "m{}", method.raw());
    }
}

fn dump_function(output: &mut String, function: &MirFunction) {
    let _ = write!(
        output,
        "function s{} f{}(",
        function.symbol.raw(),
        function.function.raw()
    );
    for (index, parameter) in function.parameters.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "t{}", parameter.raw());
    }
    output.push_str(") -> (");
    for (index, result) in function.results.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "t{}", result.raw());
    }
    output.push_str(") effects[");
    dump_effects(output, function.effects);
    output.push_str("]\n");
    dump_blocks(output, &function.blocks);
}

fn dump_nested_function(output: &mut String, function: &MirNestedFunction) {
    let _ = write!(
        output,
        "nested s{} nf{} captures ",
        function.owner.raw(),
        function.function.raw()
    );
    if function.captures.is_empty() {
        output.push('-');
    } else {
        for (index, capture) in function.captures.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            let mode = match capture.mode {
                MirCaptureMode::Value => "value",
                MirCaptureMode::Cell => "cell",
            };
            let _ = write!(
                output,
                "cap{}:bind{}@{}:t{}:{mode}",
                capture.capture.raw(),
                capture.binding.raw(),
                capture.slot,
                capture.type_id.raw()
            );
        }
    }
    output.push_str(" params(");
    dump_type_ids(output, &function.parameters);
    output.push_str(") results(");
    dump_type_ids(output, &function.results);
    output.push_str(") effects[");
    dump_effects(output, function.effects);
    output.push_str("]\n");
    dump_blocks(output, &function.blocks);
}

fn dump_blocks(output: &mut String, blocks: &[MirBlock]) {
    for block in blocks {
        let _ = write!(output, "  b{}(", block.block.raw());
        for (index, argument) in block.arguments.iter().enumerate() {
            if index != 0 {
                output.push_str(", ");
            }
            let _ = write!(
                output,
                "v{}:t{}",
                argument.value.raw(),
                argument.type_id.raw()
            );
        }
        output.push_str("):\n");
        for instruction in &block.instructions {
            if let Some(result_type) = instruction.result_type {
                let _ = write!(
                    output,
                    "    v{}:t{} = ",
                    instruction.result.raw(),
                    result_type.raw()
                );
            } else {
                let _ = write!(output, "    do v{} ", instruction.result.raw());
            }
            dump_instruction(output, &instruction.kind);
            output.push('\n');
        }
        output.push_str("    ");
        dump_terminator(output, &block.terminator);
        output.push('\n');
    }
}

fn dump_instruction(output: &mut String, instruction: &MirInstructionKind) {
    if dump_numeric_instruction(output, instruction)
        || dump_callable_or_schema_instruction(output, instruction)
    {
        return;
    }
    match instruction {
        MirInstructionKind::StringConstant(value) => {
            let _ = write!(output, "const.string {value:?}");
        }
        MirInstructionKind::BooleanConstant(value) => {
            let _ = write!(output, "const.boolean {value}");
        }
        MirInstructionKind::NilConstant => output.push_str("const.nil"),
        MirInstructionKind::FunctionReference(function) => {
            let _ = write!(output, "functionReference s{}", function.raw());
        }
        MirInstructionKind::TupleMake(values) => dump_values(output, "tupleMake", values),
        MirInstructionKind::ArrayMake {
            elements,
            element_map,
        } => {
            let map = array_element_map_name(*element_map);
            let _ = write!(output, "arrayMake {map} ");
            dump_value_list(output, elements);
        }
        MirInstructionKind::ArrayCreate {
            length,
            initial_value,
            element_map,
        } => {
            let map = array_element_map_name(*element_map);
            let _ = write!(
                output,
                "arrayCreate {map} v{} v{}",
                length.raw(),
                initial_value.raw()
            );
        }
        MirInstructionKind::TableMake {
            entries,
            key_map,
            value_map,
        } => {
            let key_map = array_element_map_name(*key_map);
            let value_map = array_element_map_name(*value_map);
            let _ = write!(output, "tableMake {key_map} {value_map} ");
            dump_table_entries(output, entries);
        }
        MirInstructionKind::ArrayGet { array, index } => {
            dump_binary(output, "arrayGet", *array, *index);
        }
        MirInstructionKind::ArrayLength { array } => {
            let _ = write!(output, "arrayLength v{}", array.raw());
        }
        MirInstructionKind::ArrayGetChecked { array, index } => {
            dump_binary(output, "arrayGetChecked", *array, *index);
        }
        MirInstructionKind::ArraySet {
            array,
            index,
            value,
            element_map,
        } => {
            let map = array_element_map_name(*element_map);
            let _ = write!(
                output,
                "arraySet {map} v{} v{} v{}",
                array.raw(),
                index.raw(),
                value.raw()
            );
        }
        MirInstructionKind::ArrayFill {
            array,
            value,
            element_map,
        } => {
            let map = array_element_map_name(*element_map);
            let _ = write!(output, "arrayFill {map} v{} v{}", array.raw(), value.raw());
        }
        binary @ (MirInstructionKind::BooleanAnd { .. }
        | MirInstructionKind::BooleanOr { .. }
        | MirInstructionKind::CompareEqual { .. }
        | MirInstructionKind::CompareNotEqual { .. }) => dump_binary_instruction(output, binary),
        MirInstructionKind::BooleanNot { operand } => dump_unary(output, "booleanNot", *operand),
        MirInstructionKind::GcSafePoint {
            safe_point, roots, ..
        } => {
            let _ = write!(output, "gcSafePoint sp{} roots ", safe_point.raw());
            dump_value_list(output, roots);
        }
        MirInstructionKind::RetainRoot { value } => {
            let _ = write!(output, "retainRoot v{}", value.raw());
        }
        MirInstructionKind::ReleaseRoot { handle } => {
            let _ = write!(output, "releaseRoot v{}", handle.raw());
        }
        MirInstructionKind::Pin { value } => {
            let _ = write!(output, "pin v{}", value.raw());
        }
        MirInstructionKind::Unpin { handle } => {
            let _ = write!(output, "unpin v{}", handle.raw());
        }
        MirInstructionKind::WriteBarrier {
            owner,
            slot,
            previous,
            value,
        } => {
            let _ = write!(
                output,
                "writeBarrier v{} slot {} previous ",
                owner.raw(),
                slot.raw()
            );
            dump_optional_value(output, *previous);
            output.push_str(" value ");
            dump_optional_value(output, *value);
        }
        _ => unreachable!("specialized MIR dumper accepts every remaining instruction"),
    }
}

const fn array_element_map_name(map: ArrayElementMap) -> &'static str {
    match map {
        ArrayElementMap::Scalar => "scalar",
        ArrayElementMap::ManagedReference => "managed",
    }
}

fn dump_numeric_instruction(output: &mut String, instruction: &MirInstructionKind) -> bool {
    if dump_numeric_binary_instruction(output, instruction) {
        return true;
    }
    match instruction {
        MirInstructionKind::IntegerConstant(value) => {
            let _ = write!(
                output,
                "const.integer {} {value}",
                integer_kind_text(value.kind())
            );
        }
        MirInstructionKind::FloatConstant(value) => {
            let _ = match value {
                FloatValue::Float32(bits) => {
                    write!(output, "const.float Float32 0x{bits:08x}")
                }
                FloatValue::Float64(bits) => {
                    write!(output, "const.float Float64 0x{bits:016x}")
                }
            };
        }
        MirInstructionKind::IntegerNegate { kind, operand } => {
            dump_numeric_unary(output, "integer.negate", integer_kind_text(*kind), *operand);
        }
        MirInstructionKind::FloatNegate { kind, operand } => {
            dump_numeric_unary(output, "float.negate", float_kind_text(*kind), *operand);
        }
        _ => return false,
    }
    true
}

fn dump_numeric_binary_instruction(output: &mut String, instruction: &MirInstructionKind) -> bool {
    let (name, kind, left, right) = match instruction {
        MirInstructionKind::CheckedIntegerAdd { kind, left, right } => {
            ("integer.checkedAdd", integer_kind_text(*kind), left, right)
        }
        MirInstructionKind::CheckedIntegerSubtract { kind, left, right } => (
            "integer.checkedSubtract",
            integer_kind_text(*kind),
            left,
            right,
        ),
        MirInstructionKind::CheckedIntegerMultiply { kind, left, right } => (
            "integer.checkedMultiply",
            integer_kind_text(*kind),
            left,
            right,
        ),
        MirInstructionKind::CheckedIntegerDivide { kind, left, right } => (
            "integer.checkedDivide",
            integer_kind_text(*kind),
            left,
            right,
        ),
        MirInstructionKind::CheckedIntegerRemainder { kind, left, right } => (
            "integer.checkedRemainder",
            integer_kind_text(*kind),
            left,
            right,
        ),
        MirInstructionKind::FloatAdd { kind, left, right } => {
            ("float.add", float_kind_text(*kind), left, right)
        }
        MirInstructionKind::FloatSubtract { kind, left, right } => {
            ("float.subtract", float_kind_text(*kind), left, right)
        }
        MirInstructionKind::FloatMultiply { kind, left, right } => {
            ("float.multiply", float_kind_text(*kind), left, right)
        }
        MirInstructionKind::FloatDivide { kind, left, right } => {
            ("float.divide", float_kind_text(*kind), left, right)
        }
        MirInstructionKind::CompareIntegerLess { kind, left, right } => {
            ("integer.compareLess", integer_kind_text(*kind), left, right)
        }
        MirInstructionKind::CompareIntegerGreater { kind, left, right } => (
            "integer.compareGreater",
            integer_kind_text(*kind),
            left,
            right,
        ),
        MirInstructionKind::CompareFloatLess { kind, left, right } => {
            ("float.compareLess", float_kind_text(*kind), left, right)
        }
        MirInstructionKind::CompareFloatGreater { kind, left, right } => {
            ("float.compareGreater", float_kind_text(*kind), left, right)
        }
        _ => return false,
    };
    dump_numeric_binary(output, name, kind, *left, *right);
    true
}

fn dump_callable_or_schema_instruction(
    output: &mut String,
    instruction: &MirInstructionKind,
) -> bool {
    match instruction {
        MirInstructionKind::CallStandard {
            function,
            arguments,
            declared_effects,
        } => {
            let _ = write!(output, "callStandard sf{} ", function.raw());
            dump_value_list(output, arguments);
            output.push_str(" effects[");
            dump_effects(output, *declared_effects);
            output.push(']');
        }
        MirInstructionKind::CallDirect {
            function,
            arguments,
            declared_effects,
            unwind,
        } => {
            let _ = write!(output, "callDirect s{} ", function.raw());
            dump_value_list(output, arguments);
            dump_call_contract(output, *declared_effects, *unwind);
        }
        MirInstructionKind::CallDirectMethod {
            method,
            arguments,
            declared_effects,
            unwind,
        } => {
            let _ = write!(output, "callDirectMethod m{} ", method.raw());
            dump_value_list(output, arguments);
            dump_call_contract(output, *declared_effects, *unwind);
        }
        MirInstructionKind::CallInterface {
            interface,
            method,
            slot,
            arguments,
            declared_effects,
            unwind,
        } => {
            let _ = write!(
                output,
                "call.interface i{} im{} slot#{} ",
                interface.raw(),
                method.raw(),
                slot
            );
            dump_value_list(output, arguments);
            dump_call_contract(output, *declared_effects, *unwind);
        }
        MirInstructionKind::CallIndirect {
            callee,
            arguments,
            declared_effects,
            unwind,
        } => {
            let _ = write!(output, "callIndirect v{} ", callee.raw());
            dump_value_list(output, arguments);
            dump_call_contract(output, *declared_effects, *unwind);
        }
        MirInstructionKind::RecordMake { record, fields } => {
            dump_fields(output, "recordMake", *record, None, fields);
        }
        MirInstructionKind::ClassMake {
            class,
            fields,
            object_map,
        } => {
            let _ = write!(output, "classMake c{} ", class.raw());
            dump_object_map(output, object_map);
            output.push(' ');
            dump_field_values(output, fields);
        }
        MirInstructionKind::RecordUpdate {
            record,
            base,
            fields,
        } => dump_fields(output, "recordUpdate", *record, Some(*base), fields),
        MirInstructionKind::FieldGet { base, field } => {
            let _ = write!(output, "fieldGet v{} field#{}", base.raw(), field.raw());
        }
        MirInstructionKind::FieldSet { base, field, value } => {
            let _ = write!(
                output,
                "fieldSet v{} field#{} v{}",
                base.raw(),
                field.raw(),
                value.raw()
            );
        }
        MirInstructionKind::UnionMake {
            union,
            case,
            arguments,
        } => {
            let _ = write!(output, "unionMake s{} case#{} ", union.raw(), case.raw());
            dump_value_list(output, arguments);
        }
        MirInstructionKind::InterfaceUpcast { value, interface } => {
            let _ = write!(
                output,
                "interface.upcast v{} i{}",
                value.raw(),
                interface.raw()
            );
        }
        MirInstructionKind::CaptureCellAllocate {
            binding,
            initial,
            value_type,
            object_map,
        } => {
            let _ = write!(
                output,
                "captureCell.allocate bind{} v{} t{} ",
                binding.raw(),
                initial.raw(),
                value_type.raw()
            );
            dump_object_map(output, object_map);
        }
        MirInstructionKind::CaptureCellLoad { cell } => {
            let _ = write!(output, "captureCell.load v{}", cell.raw());
        }
        MirInstructionKind::CaptureCellStore { cell, value } => {
            let _ = write!(output, "captureCell.store v{} v{}", cell.raw(), value.raw());
        }
        MirInstructionKind::ClosureEnvironmentAllocate {
            owner,
            function,
            captures,
            object_map,
        } => {
            let _ = write!(
                output,
                "closureEnvironment.allocate s{} nf{} ",
                owner.raw(),
                function.raw()
            );
            dump_object_map(output, object_map);
            output.push_str(" captures[");
            for (index, capture) in captures.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                let mode = match capture.mode {
                    MirCaptureMode::Value => "value",
                    MirCaptureMode::Cell => "cell",
                };
                if capture.self_reference {
                    let _ = write!(
                        output,
                        "cap{}:bind{}@{}=self:t{}:{mode}",
                        capture.capture.raw(),
                        capture.binding.raw(),
                        capture.slot,
                        capture.type_id.raw()
                    );
                } else {
                    let _ = write!(
                        output,
                        "cap{}:bind{}@{}=v{}:t{}:{mode}",
                        capture.capture.raw(),
                        capture.binding.raw(),
                        capture.slot,
                        capture.value.raw(),
                        capture.type_id.raw()
                    );
                }
            }
            output.push(']');
        }
        MirInstructionKind::CaptureLoad {
            capture,
            slot,
            mode,
        } => {
            let mode = match mode {
                MirCaptureMode::Value => "value",
                MirCaptureMode::Cell => "cell",
            };
            let _ = write!(
                output,
                "capture.load cap{} slot#{} {mode}",
                capture.raw(),
                slot
            );
        }
        MirInstructionKind::CaptureCellReference { capture, slot } => {
            let _ = write!(output, "capture.cell cap{} slot#{}", capture.raw(), slot);
        }
        MirInstructionKind::CaptureStore {
            capture,
            slot,
            value,
        } => {
            let _ = write!(
                output,
                "capture.store cap{} slot#{} v{}",
                capture.raw(),
                slot,
                value.raw()
            );
        }
        _ => return false,
    }
    true
}

fn dump_binary_instruction(output: &mut String, instruction: &MirInstructionKind) {
    let (name, left, right) = match instruction {
        MirInstructionKind::BooleanAnd { left, right } => ("booleanAnd", left, right),
        MirInstructionKind::BooleanOr { left, right } => ("booleanOr", left, right),
        MirInstructionKind::CompareEqual { left, right } => ("compareEqual", left, right),
        MirInstructionKind::CompareNotEqual { left, right } => ("compareNotEqual", left, right),
        _ => unreachable!("binary MIR dumper accepts only binary instructions"),
    };
    dump_binary(output, name, *left, *right);
}

fn dump_terminator(output: &mut String, terminator: &MirTerminator) {
    match terminator {
        MirTerminator::Missing => output.push_str("missing"),
        MirTerminator::Branch { target, arguments } => {
            let _ = write!(output, "branch b{} ", target.raw());
            dump_value_list(output, arguments);
        }
        MirTerminator::ConditionalBranch {
            condition,
            when_true,
            when_false,
        } => {
            let _ = write!(
                output,
                "condBranch v{} b{} b{}",
                condition.raw(),
                when_true.raw(),
                when_false.raw()
            );
        }
        MirTerminator::UnionSwitch {
            scrutinee,
            union,
            arms,
        } => {
            let _ = write!(
                output,
                "union.switch v{} s{} [",
                scrutinee.raw(),
                union.raw()
            );
            for (index, arm) in arms.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                let _ = write!(output, "case#{}:b{}", arm.case.raw(), arm.target.raw());
            }
            output.push(']');
        }
        MirTerminator::Return { values } => dump_values(output, "return", values),
        MirTerminator::Trap(trap) => {
            let _ = write!(output, "trap {}", trap_kind_text(trap.kind()));
        }
        MirTerminator::Panic(payload) => {
            output.push_str("panic ");
            dump_panic_payload(output, payload);
        }
        MirTerminator::ContinueUnwind(reason) => {
            output.push_str("resumeUnwind ");
            dump_unwind_reason(output, reason);
        }
        MirTerminator::Unreachable => output.push_str("unreachable"),
    }
}

fn dump_binary(output: &mut String, name: &str, left: ValueId, right: ValueId) {
    let _ = write!(output, "{name} v{} v{}", left.raw(), right.raw());
}

fn dump_unary(output: &mut String, name: &str, operand: ValueId) {
    let _ = write!(output, "{name} v{}", operand.raw());
}

fn dump_numeric_binary(output: &mut String, name: &str, kind: &str, left: ValueId, right: ValueId) {
    let _ = write!(output, "{name} {kind} v{} v{}", left.raw(), right.raw());
}

fn dump_numeric_unary(output: &mut String, name: &str, kind: &str, operand: ValueId) {
    let _ = write!(output, "{name} {kind} v{}", operand.raw());
}

const fn integer_kind_text(kind: IntegerKind) -> &'static str {
    match kind {
        IntegerKind::Int8 => "Int8",
        IntegerKind::Int16 => "Int16",
        IntegerKind::Int32 => "Int32",
        IntegerKind::Int64 => "Int64",
        IntegerKind::UInt8 => "UInt8",
        IntegerKind::UInt16 => "UInt16",
        IntegerKind::UInt32 => "UInt32",
        IntegerKind::UInt64 => "UInt64",
    }
}

const fn float_kind_text(kind: FloatKind) -> &'static str {
    match kind {
        FloatKind::Float32 => "Float32",
        FloatKind::Float64 => "Float64",
    }
}

fn dump_values(output: &mut String, name: &str, values: &[ValueId]) {
    output.push_str(name);
    output.push(' ');
    dump_value_list(output, values);
}

fn dump_value_list(output: &mut String, values: &[ValueId]) {
    output.push('(');
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "v{}", value.raw());
    }
    output.push(')');
}

fn dump_fields(
    output: &mut String,
    name: &str,
    record: SymbolId,
    base: Option<ValueId>,
    fields: &[(FieldId, ValueId)],
) {
    let _ = write!(output, "{name} s{}", record.raw());
    if let Some(base) = base {
        let _ = write!(output, " v{}", base.raw());
    }
    output.push_str(" {");
    for (index, (field, value)) in fields.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "field#{}=v{}", field.raw(), value.raw());
    }
    output.push('}');
}

fn dump_field_values(output: &mut String, fields: &[(FieldId, ValueId)]) {
    output.push('{');
    for (index, (field, value)) in fields.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "field#{}=v{}", field.raw(), value.raw());
    }
    output.push('}');
}

fn dump_table_entries(output: &mut String, entries: &[(ValueId, ValueId)]) {
    output.push('(');
    for (index, (key, value)) in entries.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "v{} => v{}", key.raw(), value.raw());
    }
    output.push(')');
}

fn dump_effects(output: &mut String, effects: MirEffectSummary) {
    for (index, effect) in effects.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        output.push_str(match effect {
            MirEffect::Allocates => "Allocates",
            MirEffect::WritesManagedReference => "WritesManagedReference",
            MirEffect::MayTrap => "MayTrap",
            MirEffect::MayUnwind => "MayUnwind",
            MirEffect::Suspends => "Suspends",
            MirEffect::UnsafeMemory => "UnsafeMemory",
            MirEffect::ForeignFunction => "ForeignFunction",
            MirEffect::AmbientIo => "AmbientIo",
            MirEffect::CompilerQuery => "CompilerQuery",
            MirEffect::GcSafePoint => "GcSafePoint",
            MirEffect::Roots => "Roots",
        });
    }
}

fn dump_call_contract(output: &mut String, effects: MirEffectSummary, unwind: MirUnwindAction) {
    output.push_str(" effects[");
    dump_effects(output, effects);
    output.push_str("] unwind ");
    match unwind {
        MirUnwindAction::Propagate => output.push_str("propagate"),
        MirUnwindAction::Cleanup(block) => {
            let _ = write!(output, "cleanup:b{}", block.raw());
        }
    }
}

fn dump_object_map(output: &mut String, map: &ObjectMap) {
    let _ = write!(output, "map[{}:", map.slot_count());
    for (index, slot) in map.reference_slots().iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        let _ = write!(output, "{}", slot.raw());
    }
    output.push(']');
}

fn dump_optional_value(output: &mut String, value: Option<ValueId>) {
    if let Some(value) = value {
        let _ = write!(output, "v{}", value.raw());
    } else {
        output.push_str("nil");
    }
}

const fn trap_kind_text(kind: pop_runtime_interface::TrapKind) -> &'static str {
    match kind {
        pop_runtime_interface::TrapKind::IntegerOverflow => "IntegerOverflow",
        pop_runtime_interface::TrapKind::DivisionByZero => "DivisionByZero",
        pop_runtime_interface::TrapKind::BoundsViolation => "BoundsViolation",
        pop_runtime_interface::TrapKind::ImpossibleState => "ImpossibleState",
    }
}

fn dump_panic_payload(output: &mut String, payload: &PanicPayload) {
    match payload.kind() {
        pop_runtime_interface::PanicKind::RuntimeInvariant => output.push_str("RuntimeInvariant"),
        pop_runtime_interface::PanicKind::OutOfMemory {
            requested_objects,
            requested_slots,
        } => {
            let _ = write!(output, "OutOfMemory({requested_objects},{requested_slots})");
        }
    }
}

fn dump_unwind_reason(output: &mut String, reason: &UnwindReason) {
    match reason {
        UnwindReason::Panic(payload) => dump_panic_payload(output, payload),
        UnwindReason::Cancellation => output.push_str("Cancellation"),
    }
}

fn unquote(value: &str) -> String {
    value
        .get(1..value.len().saturating_sub(1))
        .unwrap_or_default()
        .to_owned()
}
