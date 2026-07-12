# Garbage Collector Architecture

## Objective

Pop Lang's first managed runtime uses a precise, concurrent, generational
tracing garbage collector designed for:

- very fast allocation;
- short, bounded common pauses;
- high multicore marking throughput;
- predictable memory/CPU tuning;
- native LLVM code and a future VM;
- thousands of suspended coroutines;
- no dynamic values or general runtime reflection requirement.

“Go-level speed” is a benchmark target, not a claim granted by architecture.
The collector must be measured against current Go and other relevant runtimes on
the same workloads, hardware, live-heap sizes, allocation rates, and latency
percentiles. Pop Lang should beat its own published regression budgets before it
claims comparative performance.

## Selected design

The collector, provisionally named **Pop GC**, combines:

- thread-local allocation buffers for the fast path;
- a moving young generation for short-lived objects;
- age/size-based promotion;
- a region-based, mostly non-moving mature heap;
- parallel stop-the-world minor collections;
- concurrent snapshot-at-the-beginning marking for major collections;
- concurrent/lazy sweeping;
- card marking for mature-to-young references;
- precise compiler-generated stack and object pointer maps;
- allocation pacing, background workers, and bounded mutator assists;
- explicit memory-limit and heap-growth controllers.

This hybrid is intentionally not a copy of Go's collector. Generational
collection is valuable for Pop Lang's expected allocation-heavy game/server
workloads, while concurrent mature marking limits pauses as the live heap grows.

## Performance goals

Targets are evaluated on a versioned reference benchmark suite and hardware
profile. Initial release gates should include:

- allocation fast path: pointer bump with no global lock;
- no collection work for stack-allocated/scalar-replaced objects;
- minor GC pause P99 below 2 ms for the interactive benchmark profile;
- major transition/remark pause P99 below 2 ms and maximum below 10 ms on the
  documented server profile;
- default steady-state GC CPU below 10% on allocation-heavy server benchmarks;
- background marking that completes before the configured heap target;
- no pause proportional to total heap capacity; pauses may depend on active
  threads, roots, dirty cards, and bounded young live data;
- no unbounded finalizer, weak-reference, or Bubble-unload work in a pause.

These numbers are engineering gates, not language-semantic guarantees. Every
published result includes heap size, live set, roots, allocation rate, core
count, object graph shape, and percentile methodology.

## Heap organization

```text
ManagedHeap
  YoungGeneration
    EdenRegions
    SurvivorRegions
  MatureGeneration
    SizeClassRegions
    FreeRegionPool
  LargeObjectSpace
  SideMetadata
    MarkBitmaps
    CardTable
    RegionTable
    PinCounts
```

### Regions

The heap is divided into fixed-size aligned regions. The initial candidate is
2 MiB, selected after page/TLB/fragmentation benchmarks. Region metadata records
generation, allocation top, live bytes, size class, sweep state, and pinning.

Regions allow parallel ownership, cheap reclamation when empty, bounded card
tables, and future selective compaction without changing object semantics.

### Young generation

New traced objects normally allocate in Eden through a thread-local allocation
buffer. A thread-local fast path is:

1. load allocation pointer;
2. add aligned object size;
3. compare with buffer limit;
4. initialize header/pointer fields;
5. publish the new pointer.

Refilling a buffer acquires a chunk from the thread's/current Eden region.
Large, pinned, or policy-selected objects bypass the nursery.

At minor collection, reachable young objects copy into survivor regions or
promote to the mature heap. Age threshold, survivor occupancy, object size, and
copy cost guide promotion. Adaptive sizing keeps the amount of live young data
within the pause budget.

### Mature generation

Promoted/long-lived objects occupy size-class or variable-size mature regions.
Mature objects normally do not move. This keeps FFI, interface dispatch caches,
and concurrent marking simpler while allowing background sweep.

Fragmentation is measured per region. Version one reuses/sweeps regions and can
return completely empty regions to the OS. Selective mature compaction is
deferred until fragmentation data proves it necessary; it must use a separate
bounded design and cannot silently introduce long full-heap pauses.

### Large object space

Objects above an adaptive threshold allocate directly into page-aligned mature
large-object regions. They never copy during minor collection. Free ranges are
coalesced in the background. Large arrays expose allocation/retention metrics
because they can dominate memory even when object counts are low.

## Object representation

A normal traced object conceptually has:

```text
Object
  typeInfoPointer
  fields...
```

Mark, age, card, and most GC state live in side metadata rather than expanding
every object header. Locking/identity hash state, if later required, uses a lazy
side record or carefully specified header bits.

`TypeInfo` contains the minimum runtime-private facts required for collection:

- object size or layout-class information;
- precise pointer bitmap or generated scan function;
- array element stride/pointer map where applicable;
- nominal type/dispatch identity already required by language semantics;
- optional destructor only for runtime-internal resources, never user finalizers.

Field names, UDAs, source declarations, and public reflection data are not
required by the collector.

## Root model

Precise roots include:

- live managed references in thread stacks/registers at safe points;
- active and suspended coroutine frames;
- module/static roots;
- runtime scheduler and Bubble-context roots;
- explicit strong handles used by native runtime/FFI;
- temporary compiler-described roots around allocation/calls.

Conservative scanning is not the normal mode. It would prevent reliable moving
young collection and retain false objects.

### Stack maps and safe points

MIR identifies operations that may allocate, block, suspend, call unknown code,
or poll GC state. Backends produce precise maps from machine locations to live
managed references.

Safe points occur at:

- allocation slow paths;
- function prologues where required;
- loop backedges after a bounded amount of work;
- calls that may allocate/block;
- coroutine suspend/resume transitions;
- explicit polls inserted into long straight-line code.

The compiler verifies that no managed derived/interior pointer survives a safe
point without a recoverable base and offset representation.

Under ADR 0039, the published root set is mutable and keyed by canonical
`RootSlot`. A collecting safe point returns only after every relocated stack/
register root has been rewritten; object fields and runtime handles are updated
inside the collector. Pop object identity remains stable, but old evacuated
physical tokens become invalid. The bootstrap stable-handle profile exercises
the same API without changing tokens.

## Minor collection

Minor GC is parallel stop-the-world because copying a small bounded nursery is
usually faster than concurrent young collection and avoids pervasive read
barriers.

### Phases

1. **Request:** set the safepoint epoch and wake GC workers.
2. **Handshake:** each mutator reaches a safe point and publishes roots/TLAB top.
3. **Root scan:** workers scan stack/module/runtime roots that can point young.
4. **Remembered scan:** workers scan dirty mature cards.
5. **Evacuate:** copy reachable young objects, install forwarding pointers, and
   update roots/fields.
6. **Promote:** move policy-selected survivors into mature regions.
7. **Reclaim:** reset Eden/from-survivor regions in bulk.
8. **Resume:** publish new nursery/TLAB state and release mutators.

Work uses per-worker deques with stealing. Large pointer arrays split into scan
chunks so one object cannot serialize the entire pause.

If a major cycle is active, major workers reach a collector handshake before
young evacuation. Promoted objects are marked in the current major epoch and
their mature outgoing references enter the major mark queue before mutators
resume.

### Pause control

- Nursery capacity adapts to measured allocation rate, survival rate, copy
  bandwidth, and pause target.
- Objects larger than the copy budget bypass Eden.
- A mutator cannot accumulate an unbounded unpublished TLAB.
- Dirty-card work is bounded by card refinement and feedback to allocation
  pacing.
- If survival spikes, promotion increases before the next minor collection.

## Major collection

Major GC traces the mature graph concurrently using snapshot-at-the-beginning
(SATB) semantics.

### Phases

1. **Start handshake:** briefly stop/handshake mutators, snapshot roots, enable
   SATB barriers, and establish the mark epoch.
2. **Concurrent mark:** background workers trace mature objects while mutators
   run. Newly allocated mature objects are treated as live for the cycle.
3. **Drain/assist:** allocation debt may require bounded mutator marking work so
   allocation cannot outrun the collector indefinitely.
4. **Remark handshake:** drain thread-local SATB buffers, rescan changed roots,
   complete marking, and disable the SATB barrier.
5. **Concurrent sweep:** reclaim dead mature/large objects region by region.
6. **Controller update:** compute live size, fragmentation, observed mark rate,
   next target, and worker budget.

The collector never performs a routine full-heap stop-the-world mark or sweep.

## Write barriers

Managed reference stores use a combined barrier:

```text
function storeReference(owner, slot, value)
    local previous = slot.load()

    if majorMarking and previous:isMature() then
        satbBuffer.push(previous)
    end

    slot.store(value)

    if majorMarking and owner:isYoung() and value:isMature() then
        majorYoungBuffer.push(value)
    end

    if owner:isMature() and value:isYoung() then
        cardTable.mark(owner)
    end
end
```

This is semantic pseudocode. The optimized barrier uses fast inline generation/
address checks and calls a slow path only for full SATB buffers or special
regions.

- SATB preserves objects reachable at the major snapshot when references are
  overwritten.
- The major-young buffer preserves mature objects newly reached through young
  objects after the initial snapshot.
- Card marking remembers mature objects that may point to young objects.
- Null/non-managed stores skip the relevant work.
- Compiler barrier elimination/coalescing is legal only with a proved GC phase/
  object-age rule.
- Bulk moves use dedicated range barriers.

Fixed-array initialization and fill are bulk stores. Scalar arrays require no
reference barrier. Managed-element arrays use the precise homogeneous element
map and a range barrier or equivalent per-element combined barriers. Direct
contiguous backend access requires a scoped pin unless escape analysis proves
the array is not a managed allocation. See ADR 0034.

Read barriers are not required in the selected version-one design.

## Young/major interaction

The young generation is a root source for mature marking, but young objects are
not themselves swept by a major cycle.

- The major start snapshot scans existing young-to-mature edges.
- While major marking is active, young-to-mature stores log/shade the mature
  target through `majorYoungBuffer`.
- New young objects are live by construction for the current cycle.
- A minor collection during major marking pauses major workers at a collector
  handshake, evacuates young objects, and marks/enqueues promotions.
- Remark drains SATB and major-young buffers before declaring the mature graph
  complete.

This prevents a mature object from being reclaimed merely because it became
reachable through a young object during concurrent marking.

## Allocation pacing and memory control

The major controller uses a live-heap growth target conceptually similar to:

```text
nextTarget = liveHeap + max(minimumHeadroom,
    (liveHeap + scannableRoots) * heapGrowthPercent / 100)
```

Default `heapGrowthPercent` begins near 100 and adapts from observed mark
throughput, allocation rate, latency, and memory pressure. Higher growth spends
more memory to collect less often; lower growth saves memory with more GC CPU.

A hard `memoryLimit` overrides the growth target and reserves emergency
headroom. The controller accounts for heap, GC metadata, stacks, code, and major
native runtime allocations where measurable.

### GC workers

- Dedicated background workers run proportional to available CPU and mark debt.
- Idle runtime workers can assist without starving application work.
- The default sustained major-mark CPU budget starts near 20–25% during active
  marking and adapts.
- Mutator assists are proportional to allocation debt and individually bounded
  before yielding/scheduling.
- Single-core mode uses short cooperative slices rather than pretending work is
  concurrent.

## Coroutines and scheduler integration

Suspended coroutines store precise frame maps and are scannable without resuming.
Coroutine stacks use growable segmented/copied storage with stable logical frame
descriptors. Stack copying updates managed roots through maps, not conservative
guessing.

The scheduler participates in safepoint handshakes. A thread in foreign code is
either:

- in a no-managed-root state;
- registered with pinned/handle roots;
- executing a bounded non-preemptible transition; or
- cooperatively parked before a collection requiring its roots.

No coroutine may hide pointers in untyped memory.

## FFI, handles, and pinning

Raw managed pointers cannot be retained by foreign code across a safe point.
FFI uses:

- strong handles that the GC updates;
- weak handles only after their semantics are designed;
- scoped pins for APIs requiring stable addresses;
- copied buffers for long or asynchronous foreign ownership;
- explicitly unmanaged allocations for native-owned memory.

Pinning a young object promotes it before exposing the address. Pin counts and
duration are tracked. Excessive/long pins produce profiler warnings because they
increase mature fragmentation and complicate unloading.

## LLVM backend integration

The backend-neutral MIR operations include `allocateObject`, `allocateArray`,
`gcSafePoint`, `storeReference`, `pin`, `unpin`, and root/handle transitions.

The LLVM backend lowers safe points through LLVM statepoints/stack maps or an
equivalent verified mechanism that supports relocating young references. This
choice stays confined to the backend; MIR describes liveness and semantic GC
events, not LLVM intrinsics.

Verification tests inspect emitted stack maps and run forced-GC stress at every
eligible safe point. Optimizations cannot hide a live managed pointer from the
map or retain an untracked interior pointer.

## Future VM integration

The VM uses the same heap/collector where practical, but owns register and frame
maps directly. Bytecode verification identifies managed-reference slots at every
safe point. The VM can use cheaper root relocation because values reside in
known frames/register arrays.

GC behavior observable by the language—reachability, weak/finalizer policy,
identity, and errors—must match the native backend. Pause implementation details
need not match.

## Interaction with optimization

The compiler reduces GC load through:

- escape analysis and stack allocation;
- scalar replacement of records/tuples/small objects;
- allocation sinking and loop hoisting where semantics permit;
- unboxed primitives and specialized generics;
- closure-environment elision;
- write-barrier elimination for new/unpublished objects;
- arena-like temporary allocation inside compile-time execution only.

Optimization must preserve object identity where observable and cannot convert
a potentially escaping object to stack storage.

## Finalizers, weak references, and resurrection

User finalizers are not supported in version one. This removes resurrection,
ordering, hidden latency, and unload hazards. External resources use explicit
scope/`close` protocols with diagnostics for leaks in debug mode.

Weak references/weak maps are deferred. When designed, processing must happen
after marking with bounded pause work and explicit ephemeron semantics. The
collector does not accidentally expose weak behavior through tables.

## Bubble unloading

A `BubbleContext` can unload only after GC proves no live object, type info,
code pointer, closure, coroutine frame, callback, handle, or module root belongs
to it. Native code reclamation also waits for all threads to leave its code
ranges.

Version one does not promise unloadable native contexts. GC metadata includes
Bubble ownership from the start so the VM/future native implementation can add
unloading without changing object identity.

## Failure behavior

Allocation follows this sequence under pressure:

1. refill TLAB/region;
2. request minor or major work based on generation;
3. assist/poll until collection makes progress;
4. request OS memory within `memoryLimit`;
5. perform an emergency synchronous completion of the current cycle;
6. fail allocation with a deterministic out-of-memory panic.

The collector does not continue with partially initialized objects, silently
violate the memory limit, or invoke user code while internal heap locks are held.

## Observability and tuning

Runtime metrics include:

- allocation bytes/rate by type and source allocation site where sampled;
- live/committed/resident bytes by generation;
- TLAB refill counts;
- minor/major cycle count and phase durations;
- pause distribution, not only averages;
- survival/promotion rates;
- mark/scan throughput and GC CPU;
- dirty cards and SATB buffer pressure;
- mutator assist time;
- pinned bytes/duration;
- fragmentation and returned-to-OS bytes;
- root counts/scan time by stack/module/handle category.

The Stage-1 collector currently exposes saturating per-instance logical counters
for successful allocations, actual collection cycles (including capacity-
triggered cycles), reclaimed objects, and scanned objects. These counters make
benchmark work deterministic without claiming production byte, pause, worker,
or resident-memory telemetry that has not been implemented.

An execution trace correlates GC phases, safe-point handshakes, scheduler delays,
allocation assists, and user tasks. Tuning APIs expose `heapGrowthPercent`,
`memoryLimit`, and latency profile presets without making program correctness
depend on them.

## Correctness invariants

- Every live managed reference is either in a traced object, precise root, or
  registered handle at each safe point.
- No reachable object is reclaimed.
- Every evacuated young reference is updated before mutators resume.
- Mature-to-young stores dirty the owning card before the next minor scan can
  miss them.
- SATB buffers preserve overwritten mature references for the active snapshot.
- Young-to-mature logging preserves mature targets introduced during an active
  major cycle.
- Unpublished objects cannot become visible without initialized pointer fields
  and required barriers.
- GC metadata lookup is race-safe for every address the allocator publishes.
- Collection and Bubble unload never reclaim executable/type metadata still
  reachable by code or objects.

## Implementation stages

### Implementation ownership

[ADR 0038](./decisions/0038-modular-portable-runtime-implementation.md)
places collector storage, tracing, roots, pins, collection requests, and
statistics in `pop-runtime-collector`. The crate depends only on the
backend-neutral PLRI contract and owns no native C exports, process-global
singleton, platform arguments, or linker behavior. The native facade delegates
to a concrete collector; the MIR interpreter and future VM can compose the same
collector without importing the native ABI.

This boundary is also a performance contract: separation adds no runtime
registry, string lookup, heap allocation, or virtual dispatch to native
allocation/barrier fast paths. Production TLAB, region, statepoint, and barrier
fast paths may remain concrete and statically dispatched while preserving the
same PLRI semantics. Comparative claims still require the benchmark suite below.

The Stage-1 implementation separates `heap`, typed `access`, precise `trace`,
and PLRI `adapter` responsibilities inside the collector crate. The native
facade separately groups identity, allocation, storage, text/process adapters,
roots/safe points, failure termination, and private global composition state.

### Stage 1: precise stop-the-world collector

Build object/stack maps, TLAB allocation, regions, handles, and a simple precise
mark-sweep collector. This validates correctness infrastructure before adding
concurrency/generations.

The Milestone 3 executable bootstrap may use a safe stable-handle table instead
of raw regions/TLABs while proving maps, roots, safe-point publication,
transitive/cyclic reachability, reclamation, and deterministic allocation
failure. It must remain labeled as the bootstrap collector; TLABs/regions and
the production moving/concurrent behavior begin in the subsequent runtime
stages. See ADR 0022.

### Stage 2: moving nursery

Add card marking, parallel evacuation, promotion, adaptive nursery sizing, and
forced-minor stress tests.

The first Stage-2 deliverable is a single-mutator relocation conformance
collector: it really copies survivors, rewrites typed roots/object edges/handles,
invalidates old tokens, and proves remembered-card behavior. It is not the
production TLAB/parallel collector. Production selection also requires backend
and target relocation capability plus native ABI major version 2.

PLRI labels this implementation stage `RelocationConformance`: precise roots,
a moving nursery, and a generational card barrier are active; mature collection,
concurrent marking, and SATB are not. Mature objects are retained until later
stages. This stage cannot satisfy the `ProductionGenerational` runtime profile.

### Stage 3: concurrent mature marking

Add SATB barriers/buffers, concurrent work stealing, remark, concurrent sweep,
pacing, assists, and race/stress verification.

### Stage 4: latency and memory engineering

Optimize handshakes, root scanning, region reuse, OS page return, huge pages
where beneficial, NUMA placement, telemetry, and controller adaptation.

### Stage 5: VM and optional isolation

Share collector services with VM frames, validate backend conformance, and add
Bubble ownership/unload proof machinery if required.

## Benchmark suite

The implemented bootstrap harness emits versioned
`pop-runtime-benchmark-v1` tab-separated records. Each record names the
collector stage and workload, graph shape, root count, sample count, operations,
logical peak objects/slots, collections, reclaimed objects, elapsed nanoseconds,
per-operation nanoseconds, a named profile, target architecture/operating
system, build profile, and available parallelism. Workload tests verify the
logical counters deterministically; timing values are compared only on a named
hardware/toolchain profile.

The current Stage-1 workload inventory measures isolated scalar-object churn,
rooted reference-chain tracing, precise managed-reference arrays, scoped pins,
and capacity-triggered allocation pressure. These are regression baselines for
the stable-handle collector, not evidence for TLAB allocation, a moving nursery,
concurrency, or the release latency targets.

The suite includes:

- tiny-object allocation and immediate death;
- closure/coroutine churn;
- game-frame workload with a strict frame-time budget;
- high-throughput HTTP/RPC-style server graph;
- large mostly-live heap with low allocation;
- pointer-dense trees/graphs and pointer-sparse numeric arrays;
- high young survival and promotion storms;
- many threads and many suspended coroutines;
- large objects, fragmentation, pins, and foreign transitions;
- memory-limit pressure and out-of-memory behavior.

Compare end-to-end throughput, memory, GC CPU, P50/P95/P99/max pauses, and tail
request/frame latency. Microbenchmarks alone cannot establish a fast collector.

## Design reference

The official [Go GC guide](https://go.dev/doc/gc-guide) is used for its clear
cost model, heap-growth tradeoff, pacing concepts, and emphasis on measuring
latency sources. Pop GC is a distinct design and must be validated independently.
