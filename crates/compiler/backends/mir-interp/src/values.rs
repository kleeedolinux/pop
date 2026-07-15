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
use pop_runtime_interface::{ForeignAddress, ManagedReference, RuntimeFailure};
use pop_types::{FloatValue, IntegerValue};
use std::cell::RefCell;
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
    Task(SymbolId),
    CancellationSource(SymbolId),
    CancellationToken(SymbolId),
    TaskGroup(SymbolId),
    FfiHandle(u64),
    FfiBuffer(ManagedReference),
    FfiPointer(ForeignAddress),
    FfiFunction(u64),
    FfiNullPointerError,
    FfiAllocationError,
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

#[derive(Clone, Debug)]
pub struct MirClassValue {
    pub(crate) class: ClassId,
    pub(crate) reference: ManagedReference,
    pub(crate) fields: Rc<RefCell<Vec<(FieldId, RuntimeValue)>>>,
}

impl MirClassValue {
    pub(crate) fn new(
        class: ClassId,
        reference: ManagedReference,
        fields: Vec<(FieldId, RuntimeValue)>,
    ) -> Self {
        Self {
            class,
            reference,
            fields: Rc::new(RefCell::new(fields)),
        }
    }

    #[must_use]
    pub const fn class(&self) -> ClassId {
        self.class
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
            MirValue::FfiBuffer(reference) => Some(*reference),
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
        if let (MirValue::Class(class), Some(reference)) = (&mut self.visible, relocated) {
            class.reference = reference;
        }
        Ok(())
    }
}

impl PartialEq for MirClassValue {
    fn eq(&self, other: &Self) -> bool {
        self.class == other.class && Rc::ptr_eq(&self.fields, &other.fields)
    }
}

impl Eq for MirClassValue {}
