# Pop Garbage Collector Architecture

## Status

This document defines the target architecture for Pop Lang's production garbage collector.

It is not a claim that the current runtime already implements every mechanism described here. The design is intentionally staged so that correctness infrastructure, allocation performance, concurrency, and low-latency behavior can be validated independently.

The collector is provisionally named **Pop GC**.

---

# 1. Objective

Pop Lang needs a memory-management system that preserves the ergonomics and safety of a garbage-collected language while avoiding the traditional costs associated with a single globally shared managed heap.

The target is not merely a collector with short average pauses. The target is a runtime architecture in which the most frequent memory-management operations do not require the entire program to stop.

Pop GC is designed around the following goals:

- extremely fast allocation;
- very low and predictable latency;
- no routine global stop-the-world young-generation collection;
- no full-heap stop-the-world mark or sweep in normal operation;
- bounded global coordination points;
- high multicore marking and reclamation throughput;
- low barrier overhead;
- precise memory safety;
- predictable interaction with coroutines, native code, and FFI;
- efficient execution for games, servers, compilers, tools, and interactive applications;
- native LLVM code today and a future bytecode or native VM;
- thousands or millions of suspended coroutines;
- explicit control over memory limits and latency profiles;
- a design that can use ownership, borrowing, isolation, and region lifetime information when the compiler can prove them;
- a conventional managed programming model when the compiler cannot prove stronger lifetime properties.

The collector must not rely on dynamic values, general-purpose reflection, conservative pointer discovery, or a universal object header.

## 1.1 What “no stop-the-world” means

A literal guarantee that no thread will ever pause is unrealistic for a general-purpose managed runtime.

Threads can still pause or assist because of:

- allocation pressure;
- hard memory limits;
- coroutine scheduling;
- safepoint or epoch acknowledgement;
- local young-generation collection;
- object publication;
- pinning;
- foreign-code transitions;
- evacuation reserve exhaustion;
- operating-system scheduling;
- page faults;
- explicit synchronization in the application.

Therefore, the architectural target is more precise:

> Pop GC must not perform routine global pauses whose duration is proportional to total heap size, total live data, or the complete object graph.

Normal collection may use short global handshakes to change epochs, publish thread state, or enable and disable barriers. Those handshakes must have a fixed and tightly bounded amount of collector work. Heap tracing, sweeping, stack processing, card refinement, and most reclamation work must happen concurrently, incrementally, locally, or cooperatively.

The collector should also avoid global pauses caused by the young generation. Young-generation collection should be local to a scheduler or ownership domain whenever possible.

This design uses the term **no global STW in the common path** rather than claiming that no thread can ever be delayed.

## 1.2 Why this objective matters

A collector can have excellent average throughput and still produce poor application behavior.

A game may miss a frame because one pause exceeds its frame budget. A server may have high average request throughput while producing unacceptable P99.9 latency. A compiler or language server may feel unresponsive because the collector competes aggressively for memory bandwidth. A coroutine runtime may experience coordination spikes even when the heap itself is small.

For this reason, Pop GC is evaluated using both collector metrics and application-level metrics.

The collector must optimize:

- mutator throughput;
- tail latency;
- memory footprint;
- memory bandwidth;
- CPU consumed by collection;
- allocator scalability;
- barrier cost;
- root-processing cost;
- fragmentation;
- time spent waiting for global coordination;
- time spent in mutator assists;
- operating-system memory-return behavior.

No single metric is sufficient.

## 1.3 Performance target

“Faster than Go” or “faster than C with a runtime” is not granted by architecture.

Such statements are benchmark hypotheses.

All comparative claims must use:

- the same workload;
- the same hardware;
- the same operating system;
- equivalent compiler optimization levels;
- equivalent memory limits;
- equivalent live-heap sizes;
- equivalent allocation rates;
- equivalent thread counts;
- equivalent object graph shapes;
- equivalent latency measurement methods;
- warm and steady-state measurements;
- P50, P95, P99, P99.9, and maximum latency where relevant.

Pop GC should first beat its own versioned regression budgets. External comparisons come after the implementation is stable enough to measure honestly.

---

# 2. Design Principles

Pop GC follows several principles.

## 2.1 Do not collect what the compiler can prove does not need collection

The fastest traced allocation is an allocation that never reaches the managed heap.

The compiler should aggressively use:

- stack allocation;
- scalar replacement;
- escape analysis;
- closure-environment elimination;
- unboxed primitives;
- specialized generics;
- allocation sinking;
- allocation fusion;
- temporary arenas;
- ownership transfer;
- immutable sharing;
- region lifetime inference.

## 2.2 Keep local memory local

A globally shared heap forces global coordination.

Pop should preserve locality in the memory model:

- thread-local allocation buffers;
- scheduler-local young generations;
- coroutine-local temporary storage;
- isolated regions with a single external owner;
- explicitly shared mature objects only when sharing is required.

## 2.3 Make sharing explicit in the runtime model

The collector should distinguish:

- local mutable objects;
- isolated object graphs;
- immutable shared values;
- mutable shared values;
- pinned native-facing objects;
- large pointer-free buffers;
- unmanaged resources.

These categories have different safety and collection requirements. Treating all of them identically wastes CPU and memory.

## 2.4 Optimize for memory locality, not only instruction count

Tracing collectors often become limited by memory latency and bandwidth rather than arithmetic.

Pop GC should organize work by pages and regions so that workers scan nearby metadata and objects together.

## 2.5 Keep the common path concrete

Allocation, pointer stores, generation checks, and safepoint polls must not require:

- string lookup;
- dynamic registration;
- virtual dispatch;
- global locks;
- heap allocation;
- process-global collector discovery.

The implementation may preserve a backend-neutral runtime contract without inserting abstraction overhead into the native fast path.

## 2.6 Never hide catastrophic fallback behavior

The production collector must not silently perform a full-heap stop-the-world collection because fragmentation, marking debt, or evacuation reserve was mismanaged.

If an emergency mode exists, it must be:

- explicit;
- measurable;
- rare;
- documented;
- testable;
- visible in telemetry.

---

# 3. Selected Architecture

Pop GC combines:

- precise compiler-generated pointer maps;
- thread-local allocation buffers;
- scheduler-local moving young generations;
- isolated ownership regions;
- a shared page-centric mark-region heap;
- concurrent snapshot-at-the-beginning marking;
- concurrent or lazy sweeping;
- concurrent remembered-set refinement;
- selective concurrent evacuation;
- side metadata;
- adaptive pretenuring;
- explicit pinned and foreign-memory spaces;
- hard memory limits and adaptive heap growth;
- bounded mutator assists;
- stack watermarks and incremental root processing;
- deterministic resource management outside tracing;
- no user finalizers in the first production version.

The core heap organization is:

```text
ManagedMemory
  StackAndScalarStorage
  ScopedArenaSpace
  LocalHeaps
    LocalHeap[SchedulerId]
      EdenPages
      SurvivorPages
      LocalLargeObjects
  IsolatedRegionSpace
  SharedHeap
    SharedSmallObjectPages
    SharedMediumObjectRegions
    SharedEvacuationRegions
  LargeObjectSpace
    PointerFreeBlobs
    PointerContainingLargeObjects
  PinnedSpace
  RuntimeMetadata
    RegionTable
    PageTable
    MarkBitmaps
    ScanBitmaps
    CardTable
    RememberedSets
    ForwardingMetadata
    PinMetadata
    OwnershipMetadata
    StackMapRegistry
```

---

# 4. Memory Domains

## 4.1 Stack and scalar storage

Objects that do not escape their defining scope should not enter the managed heap.

The compiler may represent them as:

- machine registers;
- stack slots;
- scalarized fields;
- SSA values;
- inline aggregates;
- fixed-size stack buffers.

The collector sees such values only when they contain managed references that remain live at a safepoint.

The compiler must preserve observable identity. An object cannot be scalar-replaced if the program can observe that two references designate the same object and the transformation would change that result.

## 4.2 Scoped arenas

A scoped arena supports groups of objects with a common lifetime.

Example:

```text
arena frame {
    let commands = buildRenderCommands()
    let particles = updateParticles()
    render(commands, particles)
}
```

The arena uses bump allocation and bulk reclamation.

A reference into an arena must not outlive the arena. The compiler enforces this through lifetime analysis, borrowing rules, escape checks, or explicit annotations.

Arenas are especially useful for:

- game frames;
- parsers;
- request processing;
- temporary compiler IR;
- query execution;
- serialization;
- packet assembly;
- short-lived graph construction.

Arena objects normally require:

- no tracing;
- no mark bits;
- no card marking;
- no per-object reclamation;
- no finalization.

The runtime may still scan an arena if the arena contains references to other managed spaces. The arena itself is reclaimed as one unit.

## 4.3 Scheduler-local heaps

Each runtime scheduler owns a local young heap.

A local heap contains objects reachable only from:

- the owning scheduler's active threads;
- coroutines assigned to that scheduler;
- local runtime structures;
- local roots;
- local isolated regions currently owned by that scheduler.

The local heap is collected independently from other schedulers.

The critical invariant is:

> A shared object may not contain a direct reference to an object in a scheduler-local young heap.

This invariant prevents the shared heap from depending on the physical location of young local objects.

Objects may move from local memory to shared memory through an explicit publication operation.

## 4.4 Isolated regions

An isolated region contains an arbitrary object graph with exactly one external owner.

The graph may contain:

- mutable objects;
- cycles;
- internal aliasing;
- arrays;
- closures;
- trees;
- graphs.

The region can move between schedulers or coroutines by transferring ownership.

Conceptually:

```text
local object graph
    -> isolate
    -> move to another owner
```

The transfer does not require copying every object and does not require atomic reference-count operations on every internal edge.

The type system may expose capabilities similar to:

```text
local T
isolated T
shared T
borrowed T
pinned T
resource T
```

These names are illustrative. The final language syntax may differ.

The compiler should infer capabilities where possible.

## 4.5 Shared heap

Objects that are intentionally shared across schedulers belong in the shared heap.

Examples include:

- immutable global values;
- synchronized mutable state;
- concurrent data structures;
- intern tables;
- shared caches;
- actor registries;
- module metadata;
- shared closures;
- objects referenced by multiple ownership domains.

The shared heap is collected concurrently.

It uses a page-centric mark-region design with selective evacuation.

## 4.6 Large-object space

Large objects bypass copying young generations when copying would be too expensive.

The large-object space distinguishes:

- pointer-free blobs;
- large pointer arrays;
- large mixed-layout objects;
- pinned large objects.

Pointer-free blobs require no tracing after their liveness is established. They may use a simpler allocator and reclamation policy.

Large pointer arrays are divided into scan chunks so that one object cannot monopolize a worker or pause.

## 4.7 Pinned space

Objects with stable addresses belong in a separate pinned space or are promoted into pinned regions.

Pinning must be:

- explicit;
- scoped;
- counted;
- profiled;
- bounded where possible.

A young object is never exposed to native code and then moved behind its back. It is promoted or copied into a stable-address space before the pointer is exported.

Long-lived pins are reported by the profiler because they can increase fragmentation and reduce evacuation freedom.

---

# 5. Object Representation

Pop GC does not require one universal object header.

## 5.1 Page-described objects

Small-object pages should be monomorphic where practical.

A page descriptor may define:

- object size;
- alignment;
- pointer bitmap;
- scan function;
- nominal type;
- dispatch information;
- array element layout;
- generation;
- ownership category.

Objects on such a page may omit a per-object type pointer.

The physical payload uses one machine word per logical slot. The page/object
pointer map is the sole authority that interprets a word as a scalar or managed
reference; payload words do not duplicate that fact with a per-slot runtime
tag. A scalar whose bits equal a managed token remains a scalar and must never
be traced. Segmented token directories derive the token from the segment and
slot coordinate rather than storing the same token beside every entry.

This improves:

- object density;
- cache locality;
- scanning speed;
- metadata locality;
- memory footprint.

## 5.2 Objects requiring headers

A header may still be required for:

- dynamically dispatched objects;
- arrays with runtime length;
- identity hashing;
- monitor or lock state;
- forwarding state when side metadata is insufficient;
- exceptional runtime features.

The header should contain only semantically necessary information.

Mark state, age, card state, pin state, and most forwarding state should live in side metadata.

## 5.3 Reference representation

Managed references may use one of several internal representations:

- direct pointer;
- compressed offset;
- region-relative offset;
- stable handle;
- tagged capability-aware reference.

The first implementation may use direct pointers, but the ABI should avoid making this choice impossible to change.

The runtime should distinguish types such as:

```text
ObjectRef
LocalRef
SharedRef
PinnedRef
Handle
RawAddress
```

A raw address is not automatically a managed reference.

---

# 6. Allocation

## 6.1 Thread-local allocation buffers

The normal local allocation path is:

```text
new_top = alloc_top + aligned_size

if new_top <= alloc_limit:
    result = alloc_top
    alloc_top = new_top
    initialize pointer fields
    publish object
    return result

return allocation_slow_path(...)
```

The fast path requires:

- no global lock;
- no collector registry lookup;
- no virtual call;
- no atomic operation in the common local case.

## 6.2 Publication safety

An object is not visible to another thread until:

- all pointer fields are initialized;
- required ownership transitions are complete;
- required barriers have executed;
- the publication store has the required memory ordering.

The compiler may eliminate barriers for an unpublished object because no other mutator can observe it.

## 6.3 Adaptive pretenuring

Allocation sites with consistently high survival or large copy cost may allocate directly into:

- survivor space;
- isolated regions;
- shared mature pages;
- large-object space;
- pinned space.

The runtime records survival and promotion statistics by allocation site.

Pretenuring decisions must adapt over time and must not permanently classify an allocation site based on a small sample.

---

# 7. Local Young-Generation Collection

## 7.1 Purpose

The local young generation captures short-lived objects without involving the shared collector.

It uses copying collection because:

- allocation is extremely cheap;
- dead objects cost almost nothing to reclaim;
- surviving objects become compact;
- local ownership reduces synchronization;
- no global read barrier is required.

## 7.2 Collection scope

A local collection pauses only the owning scheduler's relevant execution context.

It does not stop unrelated schedulers.

Possible phases are:

1. request a local collection;
2. park or cooperate with the owning scheduler;
3. publish local roots and TLAB tops;
4. scan local roots;
5. scan local remembered references;
6. evacuate live young objects;
7. promote or isolate selected survivors;
8. reset Eden pages;
9. resume local execution.

## 7.3 Publication during local collection

A local object cannot be published directly into the shared heap.

Publication uses one of the following strategies:

- copy the reachable graph into shared memory;
- promote the graph into shared memory;
- freeze it as immutable shared data;
- transfer it as an isolated region;
- reject publication if the type is not safely transferable.

The publication operation is an explicit slow path compared with local allocation.

This is intentional. Cheap local allocation is more important than making arbitrary cross-thread sharing free.

## 7.4 Local remembered sets

Local mature objects may point to local young objects.

These edges are recorded by:

- cards;
- object remembered bits;
- precise slot logs;
- compiler-known owner information.

Shared objects never point directly into local young memory, so shared-heap scanning is not required for local collection.

## 7.5 Local large objects

Large local objects may bypass copying Eden.

A pointer-free local blob can be owned directly by the local heap or by an isolated region.

A pointer-containing large object must remain visible to local tracing.

---

# 8. Shared-Heap Collection

## 8.1 Overview

The shared heap uses concurrent snapshot-at-the-beginning marking.

The normal major cycle is:

1. begin a new mark epoch;
2. perform a short global handshake;
3. establish root and barrier state;
4. trace shared objects concurrently;
5. process roots incrementally;
6. refine remembered sets concurrently;
7. require bounded mutator assists if necessary;
8. complete marking with a bounded handshake;
9. sweep concurrently;
10. select evacuation regions;
11. evacuate selected regions concurrently or incrementally;
12. update controller targets.

No normal major cycle performs a full-heap stop-the-world mark or sweep.

## 8.2 Snapshot-at-the-beginning

During concurrent marking, overwriting a shared reference must preserve the old value if it belonged to the marking snapshot.

Conceptually:

```text
old = slot.load()

if shared_marking_active and is_shared_reference(old):
    satb_log(old)

slot.store(new)
```

The optimized implementation must keep the fast path minimal.

Thread-local buffers absorb most logging without global synchronization.

## 8.3 Newly allocated shared objects

New shared objects allocated during a mark cycle are treated as live for that cycle.

Their pointer fields must still obey publication and barrier rules.

## 8.4 Page-centric marking

Workers schedule pages rather than arbitrary individual objects whenever practical.

Conceptually:

```text
SharedPage
  layout
  markedBitmap
  pendingBitmap
  scannedBitmap
  objectSize
  generation
  ownershipClass
```

When a reference marks an object, the collector sets the appropriate page bit.

A worker processes pending marked objects in page order.

Benefits include:

- improved cache locality;
- fewer random metadata accesses;
- sequential scanning;
- easier vectorization;
- reduced queue traffic;
- better work aggregation.

Pointer-free pages require no field scanning.

## 8.5 Work stealing

Each GC worker owns local work queues.

Workers steal pages, regions, or scan chunks from one another.

Large pointer arrays are divided into chunks so that one large object cannot serialize the cycle.

## 8.6 Concurrent sweeping

Sweeping occurs page by page or region by region.

Completely empty regions may return physical pages to the operating system.

Partially free pages return slots or lines to allocation pools.

Sweep state is visible in side metadata so allocation can safely cooperate with lazy sweeping.

---

# 9. Region and Page Organization

A candidate hierarchy is:

```text
SuperRegion: 2 MiB
  Page:      32 KiB to 64 KiB
    Line:    128 to 256 bytes
      Slots
```

These values are not fixed language semantics. They must be selected through benchmark data.

## 9.1 Super-regions

Super-regions support:

- virtual-memory reservation;
- NUMA placement;
- page return;
- fragmentation accounting;
- large metadata indexing;
- evacuation-set selection.

## 9.2 Pages

Pages support:

- monomorphic layouts;
- page-centric marking;
- size classes;
- bitmap locality;
- allocator ownership;
- concurrent sweep state.

## 9.3 Lines

Lines allow reclamation inside partially live regions.

A mark-region layout can reuse free lines without requiring whole-region compaction.

## 9.4 Region states

A region may be:

```text
Free
LocalEden
LocalSurvivor
SharedAllocating
SharedMarking
SharedSweeping
EvacuationCandidate
Evacuating
Pinned
LargeObject
Quarantined
```

State transitions must be race-safe and explicitly verified.

---

# 10. Fragmentation and Selective Evacuation

## 10.1 Why evacuation is necessary

A permanently non-moving mature heap eventually faces fragmentation.

The runtime must not wait until fragmentation causes an emergency full collection.

It continuously measures:

- live bytes per region;
- free-line distribution;
- object size classes;
- pin density;
- relocation cost;
- expected reclaimed space;
- available evacuation reserve.

## 10.2 Evacuation-set selection

Only a bounded set of poorly utilized regions is selected.

Pinned or highly connected regions may be excluded.

Selection considers:

```text
benefit =
    reclaimable_bytes
    - copy_cost
    - reference_update_cost
    - reserve_pressure
    - pin_penalty
```

## 10.3 Concurrent evacuation

The preferred long-term design supports concurrent selective evacuation.

During evacuation:

- objects receive forwarding metadata;
- references are resolved through a barrier or slow path;
- new references target the relocated object;
- stale physical addresses are not exposed as stable identity;
- evacuated regions enter quarantine before reuse.

The collector may use a phase-specific read barrier rather than a permanent barrier.

Conceptually:

```text
function resolve(reference):
    if region_state(reference) != Evacuating:
        return reference

    return resolve_forwarded(reference)
```

## 10.4 Evacuation reserve

The runtime maintains enough free memory to complete planned evacuation.

A collection must not begin relocation work that cannot finish within the current reserve.

Memory-limit policy must include:

- live shared heap;
- local heaps;
- stack memory;
- GC metadata;
- evacuation reserve;
- native runtime allocations;
- pinned memory;
- large objects.

## 10.5 No hidden full compaction

If evacuation cannot proceed, the runtime may:

- reduce allocation rate;
- increase mutator assists;
- request local promotion changes;
- avoid selecting additional regions;
- return an explicit out-of-memory failure.

It must not silently convert the cycle into an unbounded full-heap pause.

---

# 11. Write Barriers

Pop GC uses barrier specialization.

There is no single universal barrier sequence for every pointer store.

## 11.1 Barrier matrix

```text
Store category                    Required action
--------------------------------------------------------------
local -> local                    usually none
local new object initialization   none before publication
isolated -> same isolated region  none
shared -> shared                  SATB during shared marking
shared -> local                   forbidden
shared -> isolated                restricted by ownership rules
shared -> young local             forbidden
local -> shared                   mark/log shared target if required
shared -> immutable shared        SATB if overwriting old shared edge
scalar store                      none
pointer-free array store          none
pinned -> shared                  shared barrier rules
bulk reference move               range barrier
```

## 11.2 Barrier elimination

The compiler may eliminate a barrier when it proves:

- the owner is unpublished;
- the slot is initialized for the first time;
- the owner and target are inside the same isolated region;
- the store cannot create an inter-generational edge;
- shared marking is impossible in the current code path;
- the stored value is null or non-managed;
- the page contains no managed references.

Barrier elimination must be verified conservatively.

Object mutability remains distinct from ownership and placement. Freezing a
shared graph first verifies its complete managed-reference closure and then
atomically marks every reached object `SharedImmutable`; all later payload
mutation fails before barrier or heap state changes. Shared ownership by itself
does not imply immutability.

Verified MIR retains a closed proof on an elided barrier rather than deleting
the safety argument. The first proof is a same-block, non-escaping
`UnpublishedOwner` allocation fact. Backends consume that verified proof and do
not infer weaker backend-local barrier rules. See
[ADR 0080](./decisions/0080-shared-immutability-and-barrier-proofs.md).

## 11.3 Remembered sets

The common store path should perform only a cheap card or page-state update.

More expensive refinement runs concurrently.

A two-stage design is:

1. dirty a coarse card;
2. refine dirty cards into precise slot or object sets.

The runtime tracks refinement debt. If debt grows too quickly, allocation pacing or bounded mutator assists must compensate.

## 11.4 Bulk operations

Array copy, fill, deserialization, and object cloning use range barriers.

A bulk primitive must never bypass collector semantics by expanding into untracked native memory operations.

---

# 12. Root Model

Precise roots include:

- active managed stack slots;
- managed references in registers;
- suspended coroutine frames;
- module and static roots;
- runtime scheduler structures;
- isolated-region owners;
- shared handles;
- temporary compiler-described roots;
- native transition roots;
- VM registers and frames in the future VM.

Conservative scanning is not the normal mode.

False positives would:

- retain dead objects;
- interfere with relocation;
- prevent reliable local collection;
- complicate FFI safety;
- weaken ownership guarantees.

---

# 13. Safepoints, Epochs, and Handshakes

## 13.1 Safepoints

Potential safepoints occur at:

- allocation slow paths;
- calls that may allocate or block;
- loop backedges after bounded work;
- coroutine suspension;
- coroutine resumption;
- foreign-code transitions;
- explicit runtime polls;
- selected function prologues;
- long-running compiler-inserted polling points.

The compiler verifies that no unmanaged interior pointer survives a safepoint unless its base object and offset can be reconstructed safely.

## 13.2 Epoch handshakes

A global phase transition uses an epoch.

Each mutator:

1. observes the new epoch;
2. reaches a poll or transition point;
3. publishes required local state;
4. acknowledges the epoch;
5. resumes or cooperates with the phase.

The coordinator must not process the entire heap while threads are stopped.

The handshake only establishes a consistent protocol state.

## 13.3 Uncooperative threads

A thread in foreign code must be in one of these states:

- detached from managed roots;
- operating only through registered handles;
- inside a bounded non-preemptible transition;
- parked before a collection requiring its state.

A foreign call may not hide a managed pointer in untyped memory across a safepoint.

---

# 14. Coroutine Integration

Suspended coroutines should not require resumption for scanning.

Ready coroutine frames waiting in scheduler queues have the same precise-root
requirement. ADR 0077 requires every non-running retained task frame to own one
collector-visible root container; the active worker stack assumes root
responsibility only after dispatch restores the container's current `RootSlot`
values.

Coroutine stacks are represented as stacklets or chunks:

```text
Coroutine
  StackChunk
    frame descriptors
    pointer bitmap
    saved registers
    next chunk
```

A suspended coroutine is quiescent.

Its stack chunks can be scanned concurrently as heap-like root containers.

## 14.1 Active-stack watermarks

Active stacks use watermarks.

At the beginning of a root-processing epoch:

- the runtime installs or advances a watermark;
- stack regions below the watermark are known to have been processed;
- the mutator or a worker incrementally processes remaining frames;
- stack mutation follows compiler-defined rules.

This avoids a pause proportional to all active and suspended stacks.

## 14.2 Coroutine migration

A coroutine cannot migrate to another scheduler while retaining direct references into the original scheduler's young heap unless one of these actions occurs:

- its reachable local graph is promoted;
- its graph becomes an isolated transferable region;
- its local references are copied;
- migration is delayed until local collection normalizes the state.

The scheduler and collector share the same ownership metadata.

---

# 15. Ownership and Type-System Assistance

The collector does not require every Pop program to use explicit ownership syntax.

However, the language and compiler should be able to prove stronger properties.

## 15.1 Local values

A local value is confined to one ownership domain.

Benefits include:

- no atomic synchronization;
- local allocation;
- local collection;
- no shared write barrier;
- easier escape analysis.

## 15.2 Isolated values

An isolated value is the unique external reference to an object graph.

It may move between owners without tracing the entire shared heap.

## 15.3 Shared values

A shared value may be referenced by multiple ownership domains.

It must obey:

- immutability; or
- synchronization requirements; or
- runtime concurrency primitives.

Shared mutable objects participate fully in shared-heap barriers.

## 15.4 Borrowed values

A borrowed reference does not own the target and cannot outlive the lender.

Borrowing helps the compiler:

- avoid heap allocation;
- prevent escape;
- avoid reference counting;
- avoid pinning;
- eliminate barriers;
- preserve local ownership.

## 15.5 Resources

External resources use deterministic lifetime management.

Examples:

- files;
- sockets;
- GPU buffers;
- operating-system handles;
- native library resources;
- database connections.

A resource type must be explicitly closed or destroyed at the end of ownership.

GC is not responsible for timely release of external resources.

---

# 16. Finalizers, Weak References, and Resurrection

User finalizers are not supported in the first production version.

They introduce:

- resurrection;
- unpredictable latency;
- ordering ambiguity;
- hidden retention;
- shutdown complexity;
- module-unload hazards;
- collector reentrancy risks.

External resources use deterministic scope and explicit close operations.

Weak references and weak maps are deferred until their semantics are fully specified.

When implemented, they require:

- explicit weak-reference processing;
- bounded pause work;
- ephemeron semantics for weak maps;
- no user code executed while heap locks are held;
- no accidental resurrection.

---

# 17. FFI, Handles, and Native Code

## 17.1 Raw pointers

Foreign code may not retain a raw pointer to a movable managed object across a safepoint.
ADR 0081 additionally requires raw foreign pointers to refer to unmanaged
storage unless the compiler creates a non-escaping lexical pin. Returning,
storing, capturing, retaining, or suspending with that scoped pointer is a
static error.

## 17.2 Handles

The runtime provides strong handles.

A handle contains:

- an index;
- a generation;
- ownership state;
- strength;
- optional pin state.

Stale handles are detected.

The collector updates handle targets after relocation.

## 17.3 Pinning

Pinning is lexical where possible:

```text
with pin(value) as pointer {
    nativeCall(pointer)
}
```

Long asynchronous native ownership should use:

- copied buffers;
- unmanaged allocations;
- explicit native-owned memory;
- stable handles.

The pin is released on every normal, expected-failure, panic, and cancellation
exit. Pin cleanup is part of canonical MIR rather than backend convention.

## 17.4 Native callbacks

Callbacks re-enter managed code through registered runtime transitions.

They must establish:

- managed-thread state;
- root publication state;
- scheduler ownership;
- safepoint participation;
- exception and panic boundaries.

Call-scoped callback values cannot escape their foreign call. An explicitly
owned callback remains rooted until deterministic close and carries an exact
thread, concurrency, blocking, reentrancy, and panic policy. Callback entry
resolves a registered managed identity; it never looks up a function by string.

---

# 18. LLVM Backend Integration

The backend-neutral intermediate representation includes operations such as:

```text
allocateObject
allocateArray
allocateInArena
allocateIsolated
publishShared
moveIsolated
gcSafePoint
storeReference
bulkStoreReference
pin
unpin
createHandle
releaseHandle
enterForeign
leaveForeign
```

The LLVM backend lowers these operations using:

- precise stack maps;
- statepoints or an equivalent verified relocation mechanism;
- concrete allocation fast paths;
- inline barrier fast paths;
- compiler-known root liveness.

The MIR expresses semantic events, not LLVM-specific intrinsics.

Compiler verification must ensure:

- every live managed reference is represented at safepoints;
- optimizations do not hide references;
- interior pointers are recoverable;
- relocation updates all required locations;
- local/shared capability transitions remain valid;
- barriers are not removed without proof.

Stress mode may force collection at every eligible safepoint.

---

# 19. Future VM Integration

A future VM uses the same semantic memory model.

The VM directly owns:

- register maps;
- frame maps;
- bytecode safepoints;
- coroutine frame layouts;
- relocation updates.

The VM may use cheaper root processing because managed references live in known register arrays and frame slots.

Native and VM backends must agree on language-observable behavior:

- reachability;
- identity;
- weak-reference semantics;
- resource behavior;
- out-of-memory behavior;
- handle behavior;
- ownership transfer;
- pinning rules.

Pause implementation details may differ.

---

# 20. Allocation Pacing and Memory Control

## 20.1 Heap-growth target

The shared collector uses a target similar to:

```text
next_target =
    live_shared_heap
    + max(
        minimum_headroom,
        weighted_live_memory * growth_percent / 100
      )
```

Weighted live memory may include:

- live shared heap;
- scannable roots;
- selected local promotion pressure;
- stack memory;
- GC metadata;
- evacuation reserve;
- pinned memory.

## 20.2 Memory limit

A hard memory limit overrides ordinary growth targets.

The limit accounts for:

- local heaps;
- shared heap;
- isolated regions;
- large-object space;
- pinned space;
- stacks;
- code;
- metadata;
- runtime-native allocations;
- evacuation reserve.

The runtime must preserve emergency headroom.

## 20.3 GC workers

Background worker count adapts to:

- allocation rate;
- mark debt;
- remembered-set debt;
- memory pressure;
- available cores;
- application latency profile.

Idle schedulers may assist collection.

GC must not permanently consume all available CPU.

## 20.4 Mutator assists

A mutator that allocates faster than the collector can reclaim memory acquires debt.

The mutator performs bounded work proportional to that debt.

Assist work must:

- be bounded per allocation slow path;
- yield after a configured budget;
- avoid unbounded latency spikes;
- appear in telemetry.

## 20.5 Single-core behavior

On one core, “concurrent” collection becomes cooperative incremental work.

The runtime uses short slices and avoids pretending that background work is free.

---

# 21. Failure Behavior

Allocation under pressure follows a defined sequence:

1. attempt TLAB allocation;
2. refill from an existing page;
3. request local collection where applicable;
4. attempt local promotion, isolation, or pretenuring;
5. request shared-cycle progress;
6. perform bounded assist work;
7. sweep or reuse available pages;
8. request operating-system memory within the limit;
9. reduce allocation pacing;
10. complete required current-cycle work synchronously in bounded slices;
11. fail with a deterministic out-of-memory panic.

The runtime must not:

- return partially initialized objects;
- exceed the hard memory limit silently;
- execute user code while internal heap locks are held;
- invalidate live handles;
- reuse quarantined memory too early;
- hide an emergency full-heap pause.

---

# 22. Safety Invariants

The following invariants are mandatory.

## 22.1 Reachability

- No reachable object is reclaimed.
- Every live managed reference is in a traced object, precise root, isolated-owner record, or registered handle.
- Every evacuated reference is updated or safely resolved before stale storage is reused.

## 22.2 Local/shared separation

- Shared objects never point directly into scheduler-local young memory.
- A local object cannot become shared without an explicit publication transition.
- Coroutine migration cannot violate local-heap ownership.
- Isolated regions have exactly one external owner.

## 22.3 Barriers

- Overwritten shared references are preserved for an active SATB epoch.
- Inter-generational local edges are recorded before a local collection can miss them.
- Bulk operations execute equivalent range barriers.
- Barrier elimination occurs only under verified compiler proofs.

## 22.4 Publication

- An object is never published with uninitialized managed pointer fields.
- Publication uses the required memory ordering.
- Ownership metadata is visible before shared references become visible.

## 22.5 Metadata

- Metadata lookup is race-safe for every published managed address.
- Region and page states transition atomically according to the state machine.
- Mark bits cannot refer to reused memory from a different allocation epoch.
- Quarantine prevents stale references from observing immediate address reuse.

## 22.6 FFI

- Raw movable pointers do not survive safepoints in foreign code.
- Pinned objects do not move while pinned.
- Handle generations detect stale handles.
- Native callbacks establish valid runtime state before accessing managed objects.

---

# 23. Observability

The runtime exposes:

- allocated bytes by domain;
- allocation rate by type and allocation site;
- stack-allocation success rate;
- scalar-replacement counts;
- arena allocation and bulk-free counts;
- local-heap size per scheduler;
- local collection count and duration;
- local survival and promotion rates;
- publication count and cost;
- isolated-region transfer count and cost;
- shared live bytes;
- committed and resident bytes;
- large-object bytes;
- pinned bytes and pin duration;
- TLAB refill counts;
- mark and scan throughput;
- page queue depth;
- bytes scanned per live byte;
- dirty-card backlog;
- remembered-set refinement time;
- SATB buffer pressure;
- mutator assist time;
- sweep time;
- evacuation-set size;
- evacuation reserve;
- forwarding slow-path counts;
- fragmentation by region;
- pages returned to the operating system;
- root counts by category;
- root-processing latency;
- epoch acknowledgement latency;
- time waiting for uncooperative foreign threads;
- GC CPU;
- memory bandwidth where measurable.

Application-facing latency metrics include:

- P50;
- P95;
- P99;
- P99.9;
- maximum;
- mutator utilization in 1 ms, 10 ms, and 100 ms windows.

A trace should correlate:

- allocations;
- local collections;
- major phases;
- handshakes;
- stack watermark progress;
- scheduler delays;
- mutator assists;
- object publication;
- region transfer;
- evacuation;
- FFI transitions;
- application tasks.

---

# 24. Performance Gates

Initial production gates should include:

- local allocation fast path is a pointer bump with no global lock;
- stack-allocated and scalar-replaced objects perform no GC allocation;
- no routine global young-generation pause;
- no routine pause proportional to total heap capacity;
- no routine full-heap stop-the-world mark or sweep;
- global epoch-transition P99 below the documented latency budget;
- local collection P99 below the profile-specific budget;
- major root transition P99 below the profile-specific budget;
- default steady-state GC CPU within the profile budget;
- background marking completes before the memory target is exhausted;
- remembered-set debt remains bounded;
- evacuation reserve remains sufficient;
- no unbounded finalizer or weak-reference processing;
- no unbounded module-unload work in a pause.

These are engineering gates, not language-level timing guarantees.

---

# 25. Implementation Stages

## Stage 1: precise stop-the-world bootstrap

Implement:

- typed heap access;
- object maps;
- stack maps;
- precise roots;
- handles;
- deterministic allocation failure;
- a simple mark-sweep collector;
- forced-GC stress tests.

This stage validates correctness only.

It is not the production architecture.

## Stage 2: production allocation infrastructure

Implement:

- regions;
- pages;
- side metadata;
- size classes;
- TLABs;
- page-described object layouts;
- pointer-free pages;
- allocation-site metrics;
- precise native stack maps.

## Stage 3: scheduler-local young heaps

Implement:

- local Eden pages;
- local survivor pages;
- local copying collection;
- local remembered sets;
- promotion;
- coroutine ownership;
- prohibition of shared-to-local references;
- publication slow paths.

This stage removes routine global young-generation pauses.

## Stage 4: shared concurrent marking

Implement:

- SATB barriers;
- thread-local SATB buffers;
- page-centric marking;
- work stealing;
- incremental root processing;
- stack watermarks;
- concurrent sweeping;
- pacing;
- bounded mutator assists.

## Stage 5: ownership and isolated regions

Implement:

- local capability inference;
- isolated-region construction;
- zero-copy ownership transfer;
- shared immutability;
- borrowing integration;
- scoped arenas;
- compiler barrier elimination from capability proofs.

## Stage 6: latency and fragmentation engineering

Implement:

- fine-grained region accounting;
- line reuse;
- evacuation-set selection;
- forwarding side metadata;
- phase-specific reference resolution;
- concurrent selective evacuation;
- pin-aware relocation;
- evacuation reserve control;
- page return and NUMA tuning.

## Stage 7: future VM and module isolation

Implement:

- VM frame maps;
- bytecode root maps;
- shared collector services;
- module ownership metadata;
- code and type-liveness proof;
- safe module unloading where supported.

---

# 26. Benchmark Suite

The benchmark suite must include:

- tiny-object allocation and immediate death;
- stack-allocation-heavy code;
- closure and coroutine churn;
- actor-style message passing;
- isolated-region transfer;
- shared mutable graphs;
- immutable shared graphs;
- game-frame workloads;
- HTTP and RPC server workloads;
- compiler and parser workloads;
- large mostly-live heaps;
- low-allocation large heaps;
- pointer-dense trees and graphs;
- pointer-sparse numeric arrays;
- high local survival;
- promotion storms;
- heavy publication;
- many schedulers;
- many suspended coroutines;
- large objects;
- fragmentation;
- pinning;
- foreign transitions;
- memory-limit pressure;
- deterministic out-of-memory behavior.

Every result records:

- collector stage;
- workload version;
- compiler version;
- target architecture;
- operating system;
- hardware profile;
- core count;
- scheduler count;
- live heap;
- committed heap;
- allocation rate;
- root count;
- object graph shape;
- memory limit;
- pause percentiles;
- application latency percentiles;
- GC CPU;
- resident memory;
- memory bandwidth where available.

Microbenchmarks alone cannot establish that the collector is fast.

---

# 27. Summary

Pop GC is not designed as a single global heap with a faster tracing loop.

Its primary architectural advantage comes from reducing how much memory must participate in global collection.

The hierarchy is:

```text
register or stack
    ↓
scoped arena
    ↓
scheduler-local young heap
    ↓
isolated transferable region
    ↓
shared concurrent heap
    ↓
pinned or unmanaged native memory
```

The shared collector remains important, but it is the final destination only for objects that genuinely require sharing.

The resulting design combines:

- GC ergonomics;
- local copying speed;
- ownership-guided isolation;
- page-centric concurrent marking;
- bounded global coordination;
- deterministic external resource management;
- selective relocation;
- precise compiler/runtime cooperation.

The success criterion is not merely that GC pauses are small.

The success criterion is that Pop programs spend most of their time executing application work, that memory-management costs remain predictable under load, and that the runtime does not need a global full-heap stop-the-world operation in normal execution.
