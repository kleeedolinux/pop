use pop_backend_mir_interp::{MirInterpreter, MirValue};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble, artifact_sha256_hex};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId, SymbolId};
use pop_runtime_collector::StableGenerationalRuntime;
use pop_runtime_interface::{
    ArrayAllocationRequest, FfiBytesBorrow, FfiBytesBorrowId, GarbageCollectorContract,
    ManagedReference, ObjectAllocationRequest, RootHandle, RootPublication, RuntimeAdapter,
    RuntimeFailure, SafePointOutcome, TableAllocationRequest, WriteBarrier,
};
use pop_source::SourceFile;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BytesBorrowEvent {
    Begin {
        owner: ManagedReference,
        borrow: FfiBytesBorrowId,
        length: u64,
    },
    End {
        owner: ManagedReference,
        borrow: FfiBytesBorrowId,
    },
}

struct TrackingStableRuntime {
    runtime: StableGenerationalRuntime,
    events: Rc<RefCell<Vec<BytesBorrowEvent>>>,
}

impl RuntimeAdapter for TrackingStableRuntime {
    fn contract(&self) -> GarbageCollectorContract {
        self.runtime.contract()
    }

    fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.runtime.allocate_object(request)
    }

    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.runtime.allocate_array(request)
    }

    fn allocate_table(
        &mut self,
        request: &TableAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.runtime.allocate_table(request)
    }

    fn ffi_bytes_borrow(
        &mut self,
        bytes: ManagedReference,
    ) -> Result<FfiBytesBorrow, RuntimeFailure> {
        let borrow = self.runtime.ffi_bytes_borrow(bytes)?;
        self.events.borrow_mut().push(BytesBorrowEvent::Begin {
            owner: bytes,
            borrow: borrow.id(),
            length: borrow.length(),
        });
        Ok(borrow)
    }

    fn ffi_bytes_end_borrow(
        &mut self,
        bytes: ManagedReference,
        borrow: FfiBytesBorrowId,
    ) -> Result<(), RuntimeFailure> {
        self.runtime.ffi_bytes_end_borrow(bytes, borrow)?;
        self.events.borrow_mut().push(BytesBorrowEvent::End {
            owner: bytes,
            borrow,
        });
        Ok(())
    }

    fn retain_root(&mut self, reference: ManagedReference) -> Result<RootHandle, RuntimeFailure> {
        self.runtime.retain_root(reference)
    }

    fn release_root(&mut self, root: RootHandle) -> Result<(), RuntimeFailure> {
        self.runtime.release_root(root)
    }

    fn safe_point(
        &mut self,
        roots: &mut RootPublication,
    ) -> Result<SafePointOutcome, RuntimeFailure> {
        self.runtime.safe_point(roots)
    }

    fn write_barrier(&mut self, barrier: WriteBarrier) -> Result<(), RuntimeFailure> {
        self.runtime.write_barrier(barrier)
    }
}

#[test]
fn interpreter_executes_and_ends_zero_and_nonzero_bytes_borrows() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/withPin.pop",
            "namespace Memory\n\
             public function inspect(bytes: Bytes): Boolean\n\
                 return Ffi.withPin(bytes, function(pointer: Ffi.OptionalReadOnlyPointer<Byte>, length: Ffi.C.Size): Boolean\n\
                     return Ffi.OptionalReadOnlyPointer.isPresent(pointer)\n\
                 end)\n\
             end\n",
        )
        .expect("source"),
    );
    let result = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![module],
        )
        .with_ffi_dependency(ffi),
    );
    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let mir = pop_mir::lower_hir_bubble_with_fingerprint(
        result.hir().expect("byte pin HIR"),
        result.types(),
        artifact_sha256_hex,
    )
    .expect("byte pin MIR");

    for (payload, expected) in [(&[][..], false), (&[1, 2, 3][..], true)] {
        let mut stable = StableGenerationalRuntime::new();
        let bytes = stable
            .allocate_immutable_bytes(payload)
            .expect("immutable bytes");
        let events = Rc::new(RefCell::new(Vec::new()));
        let runtime = TrackingStableRuntime {
            runtime: stable,
            events: events.clone(),
        };
        let interpreter = MirInterpreter::with_runtime(&mir, result.types(), runtime)
            .expect("verified byte pin MIR");

        assert_eq!(
            interpreter
                .call(SymbolId::from_raw(0), &[MirValue::Bytes(bytes)])
                .expect("byte pin execution"),
            vec![MirValue::Boolean(expected)]
        );

        let events = events.borrow();
        let [
            BytesBorrowEvent::Begin {
                owner: begin_owner,
                borrow: begin_borrow,
                length,
            },
            BytesBorrowEvent::End {
                owner: end_owner,
                borrow: end_borrow,
            },
        ] = events.as_slice()
        else {
            panic!("expected one exact begin/end pair, found {events:?}");
        };
        assert_eq!(*begin_owner, bytes);
        assert_eq!(*end_owner, bytes);
        assert_eq!(*begin_borrow, *end_borrow);
        assert_eq!(*length, payload.len() as u64);
    }
}
