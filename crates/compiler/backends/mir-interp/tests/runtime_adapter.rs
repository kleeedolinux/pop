use pop_backend_mir_interp::{
    ExecutionError, MirInterpreter, MirValue, ReferenceRuntimeAdapter, ReferenceRuntimeEvent,
};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_mir::{lower_hir_bubble, parse_mir_dump};
use pop_runtime_interface::{
    ArrayAllocationRequest, CollectionStatistics, GarbageCollectorContract, ManagedReference,
    ObjectAllocationRequest, PanicKind, PanicPayload, RootHandle, RootPublication, RuntimeAdapter,
    RuntimeFailure, SafePointOutcome, Trap, TrapKind, UnwindReason, WriteBarrier,
};
use pop_runtime_native::{BootstrapRuntime, HeapLimits};
use pop_source::SourceFile;

fn lower(text: &str) -> (pop_mir::MirBubble, pop_types::TypeArena) {
    let source = SourceFile::new(FileId::from_raw(0), "src/runtime.pop", text).expect("source");
    let front_end = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(0),
        NamespaceId::from_raw(0),
        Vec::new(),
        vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
    ));
    assert!(
        front_end.diagnostics().is_empty(),
        "{}",
        front_end.diagnostic_snapshot()
    );
    let mir = lower_hir_bubble(front_end.hir().expect("HIR"), front_end.types()).expect("MIR");
    (mir, front_end.types().clone())
}

#[derive(Default)]
struct RecordingRuntime {
    inner: ReferenceRuntimeAdapter,
    allocations: usize,
    safe_points: usize,
    barriers: usize,
    retained: usize,
    released: usize,
}

impl RuntimeAdapter for RecordingRuntime {
    fn contract(&self) -> GarbageCollectorContract {
        self.inner.contract()
    }

    fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.allocations += 1;
        self.inner.allocate_object(request)
    }

    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.allocations += 1;
        self.inner.allocate_array(request)
    }

    fn retain_root(&mut self, reference: ManagedReference) -> Result<RootHandle, RuntimeFailure> {
        self.retained += 1;
        self.inner.retain_root(reference)
    }

    fn release_root(&mut self, root: RootHandle) -> Result<(), RuntimeFailure> {
        self.released += 1;
        self.inner.release_root(root)
    }

    fn safe_point(&mut self, roots: &RootPublication) -> Result<SafePointOutcome, RuntimeFailure> {
        self.safe_points += 1;
        self.inner.safe_point(roots)
    }

    fn write_barrier(&mut self, barrier: WriteBarrier) -> Result<(), RuntimeFailure> {
        self.barriers += 1;
        self.inner.write_barrier(barrier)
    }
}

#[test]
fn interpreter_routes_allocations_and_safe_points_through_an_injected_runtime() {
    let (mir, types) = lower(
        "namespace Main\n\
         public function run(): Int\n\
             local first: {Int} = { 1 }\n\
             local second: {Int} = { 2 }\n\
             return 7\n\
         end\n",
    );
    let interpreter = MirInterpreter::with_runtime(&mir, &types, RecordingRuntime::default())
        .expect("verified MIR");
    assert_eq!(
        interpreter.call(mir.functions()[0].symbol(), &[]),
        Ok(vec![MirValue::Integer(
            pop_types::IntegerValue::parse_decimal("7", pop_types::IntegerKind::Int64)
                .expect("integer")
        )])
    );
    let runtime = interpreter.runtime();
    assert_eq!(runtime.allocations, 2);
    assert_eq!(runtime.safe_points, 2);
}

#[test]
fn reference_runtime_records_canonical_allocation_and_precise_root_events() {
    let (mir, types) = lower(
        "namespace Main\n\
         public function keepFirst(): {Int}\n\
             local first: {Int} = { 1 }\n\
             local second: {Int} = { 2 }\n\
             return first\n\
         end\n",
    );
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified MIR");
    interpreter
        .call(mir.functions()[0].symbol(), &[])
        .expect("execute allocation trace");
    let runtime = interpreter.runtime();
    assert_eq!(runtime.events().len(), 4);
    assert!(matches!(
        &runtime.events()[0],
        ReferenceRuntimeEvent::SafePoint { roots, .. } if roots.is_empty()
    ));
    assert!(matches!(
        runtime.events()[1],
        ReferenceRuntimeEvent::AllocateArray {
            length: 1,
            element_map: pop_runtime_interface::ArrayElementMap::Scalar,
            ..
        }
    ));
    assert!(matches!(
        &runtime.events()[2],
        ReferenceRuntimeEvent::SafePoint { roots, .. }
            if roots == &[ManagedReference::new(1)]
    ));
    assert!(matches!(
        runtime.events()[3],
        ReferenceRuntimeEvent::AllocateArray { length: 1, .. }
    ));
}

#[test]
fn managed_field_mutation_routes_the_explicit_write_barrier() {
    let (mir, types) = lower(
        "namespace Main\n\
         public class Holder\n\
             public values: {Int}\n\
             public function Holder.new(values: {Int}): Holder\n\
                 return Holder { values = values }\n\
             end\n\
             public function Holder:set(values: {Int})\n\
                 self.values = values\n\
             end\n\
         end\n\
         public function run()\n\
             local holder = Holder.new({ 1 })\n\
             holder:set({ 2 })\n\
         end\n",
    );
    let symbol = mir
        .functions()
        .iter()
        .find(|function| function.results().is_empty())
        .expect("run")
        .symbol();
    let interpreter = MirInterpreter::with_runtime(&mir, &types, RecordingRuntime::default())
        .expect("verified MIR");
    assert_eq!(interpreter.call(symbol, &[]), Ok(Vec::new()));
    assert_eq!(interpreter.runtime().barriers, 1);
}

#[test]
fn interpreter_reports_portable_traps_and_panic_unwinds() {
    let types = pop_types::TypeArena::new();
    let trap = parse_mir_dump(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> () effects[MayTrap]\n  b0():\n    trap DivisionByZero\n",
    )
    .expect("trap MIR");
    assert_eq!(
        MirInterpreter::new(&trap, &types)
            .expect("verified trap")
            .call(trap.functions()[0].symbol(), &[]),
        Err(ExecutionError::Runtime(RuntimeFailure::Trap(Trap::new(
            TrapKind::DivisionByZero
        ))))
    );

    let panic = parse_mir_dump(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> () effects[MayUnwind]\n  b0():\n    panic RuntimeInvariant\n",
    )
    .expect("panic MIR");
    assert_eq!(
        MirInterpreter::new(&panic, &types)
            .expect("verified panic")
            .call(panic.functions()[0].symbol(), &[]),
        Err(ExecutionError::Runtime(RuntimeFailure::Unwind(
            UnwindReason::Panic(PanicPayload::new(PanicKind::RuntimeInvariant))
        )))
    );
}

#[test]
fn panic_capable_calls_propagate_or_enter_their_verified_cleanup_edge() {
    let types = pop_types::TypeArena::new();
    let mir = parse_mir_dump(concat!(
        "mir bubble b0 namespace n0\n",
        "dependencies\n",
        "function s0 f0() -> () effects[MayUnwind]\n",
        "  b0():\n",
        "    panic RuntimeInvariant\n",
        "function s1 f1() -> () effects[MayUnwind]\n",
        "  b0():\n",
        "    do v0 callDirect s0 () effects[MayUnwind] unwind propagate\n",
        "    return ()\n",
        "function s2 f2() -> () effects[MayUnwind]\n",
        "  b0():\n",
        "    do v0 callDirect s0 () effects[MayUnwind] unwind cleanup:b1\n",
        "    return ()\n",
        "  b1():\n",
        "    return ()\n",
        "function s3 f3() -> () effects[MayUnwind]\n",
        "  b0():\n",
        "    resumeUnwind RuntimeInvariant\n",
    ))
    .expect("unwind MIR");
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified unwind MIR");
    let panic = ExecutionError::Runtime(RuntimeFailure::Unwind(UnwindReason::Panic(
        PanicPayload::new(PanicKind::RuntimeInvariant),
    )));

    assert_eq!(
        interpreter.call(mir.functions()[1].symbol(), &[]),
        Err(panic.clone())
    );
    assert_eq!(
        interpreter.call(mir.functions()[2].symbol(), &[]),
        Ok(Vec::new())
    );
    assert_eq!(
        interpreter.call(mir.functions()[3].symbol(), &[]),
        Err(panic)
    );
}

#[test]
fn explicit_root_actions_use_runtime_root_handles() {
    let mut types = pop_types::TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let array = types
        .intern(pop_types::SemanticType::Array(integer))
        .expect("array");
    let text = format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0() -> () effects[Allocates,MayUnwind,GcSafePoint,Roots]\n  b0():\n    do v0 gcSafePoint sp0 roots ()\n    v1:t{array} = arrayMake scalar ()\n    do v2 retainRoot v1\n    do v3 releaseRoot v1\n    return ()\n",
        array = array.raw(),
    );
    let mir = parse_mir_dump(&text).expect("root action MIR");
    let interpreter = MirInterpreter::with_runtime(&mir, &types, RecordingRuntime::default())
        .expect("verified roots");
    assert_eq!(
        interpreter.call(mir.functions()[0].symbol(), &[]),
        Ok(Vec::new())
    );
    let runtime = interpreter.runtime();
    assert_eq!(runtime.retained, 1);
    assert_eq!(runtime.released, 1);
}

struct ForcingBootstrap {
    inner: BootstrapRuntime,
    collections: Vec<CollectionStatistics>,
}

impl ForcingBootstrap {
    fn new() -> Self {
        Self {
            inner: BootstrapRuntime::new(),
            collections: Vec::new(),
        }
    }
}

impl RuntimeAdapter for ForcingBootstrap {
    fn contract(&self) -> GarbageCollectorContract {
        self.inner.contract()
    }

    fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.inner.allocate_object(request)
    }

    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.inner.allocate_array(request)
    }

    fn retain_root(&mut self, reference: ManagedReference) -> Result<RootHandle, RuntimeFailure> {
        self.inner.retain_root(reference)
    }

    fn release_root(&mut self, root: RootHandle) -> Result<(), RuntimeFailure> {
        self.inner.release_root(root)
    }

    fn safe_point(&mut self, roots: &RootPublication) -> Result<SafePointOutcome, RuntimeFailure> {
        self.inner.request_collection();
        let outcome = self.inner.safe_point(roots)?;
        if let Some(statistics) = outcome.collection() {
            self.collections.push(statistics);
        }
        Ok(outcome)
    }

    fn write_barrier(&mut self, barrier: WriteBarrier) -> Result<(), RuntimeFailure> {
        self.inner.write_barrier(barrier)
    }
}

#[test]
fn reference_and_bootstrap_adapters_agree_while_forced_gc_reclaims_dead_allocations() {
    let (mir, types) = lower(
        "namespace Main\n\
         public function run(): Int\n\
             local first: {Int} = { 1 }\n\
             local second: {Int} = { 2 }\n\
             return 7\n\
         end\n",
    );
    let symbol = mir.functions()[0].symbol();
    let reference = MirInterpreter::new(&mir, &types)
        .expect("reference interpreter")
        .call(symbol, &[]);
    let bootstrap = MirInterpreter::with_runtime(&mir, &types, ForcingBootstrap::new())
        .expect("bootstrap interpreter");
    let bootstrap_result = bootstrap.call(symbol, &[]);

    assert_eq!(bootstrap_result, reference);
    let runtime = bootstrap.runtime();
    assert_eq!(runtime.collections.len(), 2);
    assert_eq!(runtime.collections[1].reclaimed_objects(), 1);
    assert_eq!(runtime.inner.object_count(), 1);
}

#[test]
fn interpreter_preserves_deterministic_out_of_memory_panic_unwinds() {
    let (mir, types) = lower(
        "namespace Main\n\
         public function allocate(): {Int}\n\
             return { 1 }\n\
         end\n",
    );
    let interpreter = MirInterpreter::with_runtime(
        &mir,
        &types,
        BootstrapRuntime::with_limits(HeapLimits::new(0, 0)),
    )
    .expect("verified allocation MIR");
    let expected = Err(ExecutionError::Runtime(RuntimeFailure::Unwind(
        UnwindReason::Panic(PanicPayload::new(PanicKind::OutOfMemory {
            requested_objects: 1,
            requested_slots: 1,
        })),
    )));
    let symbol = mir.functions()[0].symbol();

    assert_eq!(interpreter.call(symbol, &[]), expected);
    assert_eq!(interpreter.call(symbol, &[]), expected);
    assert_eq!(interpreter.runtime().object_count(), 0);
}
