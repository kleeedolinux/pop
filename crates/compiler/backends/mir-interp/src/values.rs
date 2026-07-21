//! Interpreter-visible values and their backend-private runtime representation.
//!
//! `MirValue` is the stable observation surface used by differential tests.
//! `RuntimeValue` additionally carries PLRI managed-reference state while an
//! execution is active; it must never leak into canonical MIR.
use crate::interpreter::ExecutionError;
use pop_foundation::{
    BuiltinTypeId, ClassId, EnumCaseId, ErrorCaseId, ErrorId, FieldId, IterationCaseId,
    ResultCaseId, SymbolId, UnionCaseId,
};
use pop_mir::MirViewKind;
use pop_runtime_interface::{ForeignAddress, ManagedReference, RuntimeFailure};
use pop_types::{CodecErrorReason, FloatValue, IntegerValue};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MirValue {
    Nil,
    Boolean(bool),
    Integer(IntegerValue),
    Float(FloatValue),
    String(String),
    Tuple(Vec<Self>),
    Array(Vec<Self>),
    List(Vec<Self>),
    Range {
        first: IntegerValue,
        last: IntegerValue,
        step: IntegerValue,
    },
    Table(Vec<(Self, Self)>),
    Function(SymbolId),
    CodecSchema(SymbolId),
    CodecWriter(MirCodecWriter),
    CodecReader(MirCodecReader),
    CodecError(MirCodecError),
    Task(SymbolId),
    CancellationSource(SymbolId),
    CancellationToken(SymbolId),
    TaskGroup(SymbolId),
    FfiHandle(u64),
    FfiBuffer(ManagedReference),
    Bytes(ManagedReference),
    View(MirViewValue),
    FfiPointer(ForeignAddress),
    FfiFunction(u64),
    FfiRegisteredCallback {
        registration: u64,
        reference: ManagedReference,
    },
    FfiNullPointerError,
    FfiAllocationError,
    FfiCallbackOpenError,
    FfiCallbackInUseError,
    FfiCallbackClosedError,
    Enum {
        definition: SymbolId,
        case: EnumCaseId,
        discriminant: u32,
    },
    Record {
        record: SymbolId,
        fields: Vec<(FieldId, Self)>,
    },
    Class(MirClassValue),
    Union {
        union: SymbolId,
        case: UnionCaseId,
        arguments: Vec<Self>,
    },
    Result {
        definition: BuiltinTypeId,
        case: ResultCaseId,
        arguments: Vec<Self>,
    },
    Iteration {
        definition: BuiltinTypeId,
        case: IterationCaseId,
        arguments: Vec<Self>,
    },
    Error {
        error: ErrorId,
        case: ErrorCaseId,
        arguments: Vec<Self>,
    },
}

pub type MirCodecError = CodecErrorReason;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MirCodecEvent {
    RecordStart(u16),
    Member {
        ordinal: u16,
        label: String,
    },
    RecordEnd,
    EnumCase {
        ordinal: u16,
        label: String,
        discriminant: u32,
    },
    UnionStart {
        ordinal: u16,
        label: String,
        payload_count: u16,
    },
    Payload(u16),
    UnionEnd,
    TupleStart(u16),
    Element(u16),
    TupleEnd,
    SequenceStart(u32),
    SequenceEnd,
    OptionalAbsent,
    OptionalPresent,
    Boolean(bool),
    Integer(IntegerValue),
    Float(FloatValue),
    String(String),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MirCodecWriter(Rc<RefCell<Vec<MirCodecEvent>>>);

impl MirCodecWriter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    #[must_use]
    pub fn events(&self) -> Vec<MirCodecEvent> {
        self.0.borrow().clone()
    }
    pub(crate) fn append_within_limit(
        &self,
        mut events: Vec<MirCodecEvent>,
        maximum_events: usize,
    ) -> bool {
        let mut stored = self.0.borrow_mut();
        if stored
            .len()
            .checked_add(events.len())
            .is_none_or(|count| count > maximum_events)
        {
            return false;
        }
        stored.append(&mut events);
        true
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirCodecReader {
    pub(crate) events: Rc<Vec<MirCodecEvent>>,
    pub(crate) position: Rc<Cell<usize>>,
}

impl MirCodecReader {
    #[must_use]
    pub fn new(events: Vec<MirCodecEvent>) -> Self {
        Self {
            events: Rc::new(events),
            position: Rc::new(Cell::new(0)),
        }
    }
}

/// Backend-private relocation-safe view descriptor.
///
/// It retains the typed lender value and stores checked offsets only; it never
/// caches an interior payload address or a callee-local SSA identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirViewValue {
    pub(crate) kind: MirViewKind,
    pub(crate) lender: MirViewLenderValue,
    pub(crate) byte_offset: usize,
    pub(crate) byte_length: usize,
    pub(crate) scalar_length: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MirViewLenderValue {
    Bytes(ManagedReference),
    Text(Rc<str>),
}

#[derive(Clone, Debug)]
pub struct MirClassValue {
    pub(crate) class: ClassId,
    pub(crate) definition: pop_types::CanonicalNominalIdentity,
    pub(crate) reference: ManagedReference,
    pub(crate) fields: Rc<RefCell<Vec<(FieldId, RuntimeValue)>>>,
}

impl MirClassValue {
    pub(crate) fn new(
        class: ClassId,
        definition: pop_types::CanonicalNominalIdentity,
        reference: ManagedReference,
        fields: Vec<(FieldId, RuntimeValue)>,
    ) -> Self {
        Self {
            class,
            definition,
            reference,
            fields: Rc::new(RefCell::new(fields)),
        }
    }

    #[must_use]
    pub const fn class(&self) -> ClassId {
        self.class
    }

    #[must_use]
    pub const fn definition(&self) -> &pop_types::CanonicalNominalIdentity {
        &self.definition
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeValue {
    pub(crate) visible: MirValue,
    pub(crate) reference: Option<ManagedReference>,
}

impl RuntimeValue {
    pub(crate) fn visible(visible: MirValue) -> Self {
        let reference = match &visible {
            MirValue::Class(class) => Some(class.reference),
            MirValue::FfiBuffer(reference)
            | MirValue::Bytes(reference)
            | MirValue::FfiRegisteredCallback { reference, .. } => Some(*reference),
            MirValue::View(MirViewValue {
                lender: MirViewLenderValue::Bytes(reference),
                ..
            }) => Some(*reference),
            _ => None,
        };
        Self { visible, reference }
    }

    pub(crate) const fn managed(visible: MirValue, reference: ManagedReference) -> Self {
        Self {
            visible,
            reference: Some(reference),
        }
    }

    pub(crate) fn install_relocated_reference(
        &mut self,
        relocated: Option<ManagedReference>,
    ) -> Result<(), ExecutionError> {
        if self.reference.is_some() != relocated.is_some() {
            return Err(ExecutionError::Runtime(RuntimeFailure::runtime_invariant()));
        }
        self.reference = relocated;
        if let Some(reference) = relocated {
            match &mut self.visible {
                MirValue::Class(class) => class.reference = reference,
                MirValue::FfiBuffer(found) | MirValue::Bytes(found) => *found = reference,
                MirValue::FfiRegisteredCallback {
                    reference: found, ..
                } => *found = reference,
                MirValue::View(MirViewValue {
                    lender: MirViewLenderValue::Bytes(found),
                    ..
                }) => *found = reference,
                _ => {}
            }
        }
        Ok(())
    }
}

impl PartialEq for MirClassValue {
    fn eq(&self, other: &Self) -> bool {
        self.definition == other.definition && Rc::ptr_eq(&self.fields, &other.fields)
    }
}

impl Eq for MirClassValue {}
