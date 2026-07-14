# Portable Runtime Collector

`pop-runtime-collector` owns heap storage, precise tracing, roots, pins,
collection requests, limits, and statistics. `BootstrapRuntime` implements the
Stage-1 stable-handle collector. `RelocationRuntime` implements the first
single-mutator Stage-2 conformance slice: it copies live young objects, updates
typed roots, object edges, strong handles, and pins, invalidates old tokens,
promotes deterministically, and maintains remembered cards.

`GenerationalRuntime` composes that moving nursery with a modular incremental
mature-heap conformance slice. Its `generational` modules separate PLRI
adaptation, SATB/publication barriers, cycle state, bounded mark/sweep work,
page/TLAB allocation, memory control, typed epoch coordination, and opt-in host
workers. Typed object ownership is stored separately from generation, page
placement, allocation class, and pin state. Explicit publication walks a
complete scheduler-local graph, prepares shared placements transactionally,
then changes ownership; shared-to-local stores fail before any SATB/card or heap
mutation. The bounded coordinator registers managed and foreign execution
states, collects exact once-only root/TLAB/barrier publications, and completes
protocol epochs without heap tracing in the handshake. The generational runtime
uses that coordinator to hold major marking idle until every registered managed
mutator has published a validated precise-root snapshot; only the final
acknowledgement enables SATB marking and makes work eligible for dispatch.
Nursery relocation remains deferred while such a snapshot contains physical
tokens. Persistent named worker
threads receive immutable precise-slot snapshots through bounded per-worker
queues, keep owner work FIFO, steal peer work from the opposite end, scan exact
object maps and remembered cards in parallel, and return sequence-ordered
results for collector-owned mutation. Large pointer and mixed
layouts advance through one bounded scan-range continuation at a time, so
neither discovery nor a worker job scales with the complete pointer array;
pointer-free large objects perform no field tracing after liveness is
established. Refined cards become
precise young roots immediately inside the collecting safe point, where no
mutator store can invalidate the snapshot. Mature sweeping advances through the
ordered heap by a bounded cursor; the mark/sweep transition builds no heap-sized
unreachable-object inventory, and allocations during sweeping are live for that
cycle. It preserves snapshot edges, shades roots, pins, and new mature objects,
removes dead placements without rescanning the page inventory for every object,
reclaims empty pages once when the sweep completes, and defers nursery
relocation while a major snapshot still contains physical tokens. The
generational composition deliberately continues to report
`RelocationConformance`: epochs/workers are not yet integrated with native
scheduler transitions, and worker batches currently join each bounded collector
slice rather than tracing concurrently with mutator execution, so
`ProductionConcurrentGenerational` cannot yet be selected.

`StableGenerationalRuntime` is the closed ADR 0059 native composition. It maps
ABI 1 nursery-eligible requests into stable mature placement, exposes the typed
array/object/table access required by the native facade, and reports
`NativeStableGenerationalConformance`. Exact-layout mature allocations use a
scheduler-keyed active-page index with one mutator-local authoritative cursor;
central page metadata changes only when that active page switches or fills.
Atomic object construction and scalar or managed-array bulk construction write
the complete precise payload before publication. Two-slot payloads stay inline,
and monotonically assigned managed tokens index deterministic sliding segment
directories for both objects and placements instead of one ordered-tree node
per allocation. The stable-only reference barrier preserves SATB/post-scan
shading while omitting the impossible mature-to-young card path. This wrapper
never invokes nursery relocation or selective evacuation; those remain gated on
ABI 2 writable-root proof.

The same conformance runtime now records concrete Stage-2 allocation placement:
validated region/page/TLAB geometry, monomorphic page descriptors with precise
pointer layouts, scheduler-indexed Eden pointer bumps and TLAB cursors, separate
mature, large, and pinned domains, survivor-copy placement, deterministic
promotion, and immediate pinned-space placement. Physical regions never mix
allocation domains or scheduler-local owners. Their immutable telemetry reports
capacity, committed/live/free/fragmented bytes, pages, objects, precise
reference slots, pinned bytes, and pin density; shared regions follow explicit
allocating/marking/sweeping states. Deterministic evacuation selection excludes
pinned and large regions, rejects non-positive estimated benefit, counts
already selected regions against its bound, and admits live-copy cost only when
it fits the protected evacuation reserve. Selected regions leave allocation
pools until evacuation or explicit cancellation. The implemented
stopped-mutator evacuation slice validates every precise reference before
mutation and assigns private forwarding tokens. When configured, the collector
stages selected-object copies and the persistent bounded worker pool rewrites
their internal edges. Results return in submission order before the collector
rewrites outside fields, stack roots, strong handles, and card metadata. It then
places copies into compact monomorphic shared pages, invalidates old tokens, and
passes retired regions through quarantine before removing their pages.
Placement and heap state are staged together, so stale roots, malformed
metadata, worker failure, or peak evacuation-reserve exhaustion cannot expose a
partial relocation. Workers may be attached once to a runtime that already has
custom allocation/memory policy. This is parallel stopped-mutator reference
rewrite work, not parallel copying or mutator-concurrent evacuation;
phase-specific reference resolution and concurrent relocation remain
unfinished production work. A separate memory controller
enforces a byte hard limit before heap mutation, protects emergency and
evacuation reserves, accounts
typed stack/code/metadata/native/arena/isolated usage, adapts the collection
target with sixteen MiB of default startup headroom, performs bounded
mature-cycle assists, returns empty logical pages, and
reports domain/debt/pressure/OOM telemetry. These logical descriptors validate
ownership and allocation transitions without exposing a raw address through
PLRI. Parallel per-scheduler TLAB ownership, virtual-memory reservation,
size-class reuse, adaptive worker sizing and stealing policy, concurrent card
refinement/lazy sweeping, and measured production fast paths remain required
before the production profile.

Scoped pin metadata counts handles separately from uniquely pinned objects and
tracks age in deterministic safe-point units. A configurable threshold reports
each long-lived handle once; completed and currently active maximum ages remain
observable without wall-clock dependence or heap-content reflection. The first
pin performs constant-time token preflight and page-placement transition rather
than cloning heap metadata, while additional handles reuse the stable pinned
placement.

This crate is reusable by native execution, the MIR interpreter, and a future
VM. It contains no C exports, native symbol mapping, platform process adapters,
linker policy, or process-global singleton. See
[ADR 0038](../../../architecture/decisions/0038-modular-portable-runtime-implementation.md).

The bootstrap implementation is divided into `heap`, `access`, `trace`, and
`adapter` modules. The `relocation` directory separately groups its allocation,
heap, collection, and adapter ownership; `generational` groups mature-cycle state,
mark/sweep work, barriers, page/TLAB allocation, memory control, coordination,
bounded workers, and its adapter. The allocation, coordination, memory, and
worker submodules separate public typed descriptors from mutable state.
These are static Rust partitions behind the same PLRI dependency, not runtime
plugins or dynamic dispatch.

The `generational::coordination` partition separates typed epoch/publication
vocabulary, its deterministic state machine, and runtime integration. Detached
and handle-only mutators acknowledge automatically; managed mutators publish
precise state; bounded foreign transitions remain pending until they enter a
safe state. Registered mutators gate the major mark snapshot and worker
eligibility. This remains host conformance infrastructure, not a claim that
background collection or native scheduler handshakes are complete.

The `generational::workers` partition owns persistent host threads, bounded
owner-FIFO queues with opposite-end peer stealing, immutable bounded mark-slot
snapshots, deterministic result ordering, telemetry, and joined shutdown. It
performs parallel marking,
remembered-card refinement, and sweep dispatch only when explicitly configured;
selected-region evacuation can also submit one internal-edge-rewrite job per
collector-staged selected-object copy while retaining a collector-owned atomic
commit. It does not claim adaptive sizing or stealing policy,
mutator-concurrent tracing/refinement, or concurrent heap mutation. Major
telemetry records
completed large-object scan chunks, the maximum slots per chunk, pending chunk
queue depth, and pointer-free large objects without exposing heap contents.

The ownership foundation implements scheduler-local/shared publication and
isolated regions as separate mechanisms. Isolation verifies a unique registered
owner and rejects other handles, pins, stack roots, or outside incoming edges
before transactionally assigning a distinct region and placement. Transfer
changes only the owner scheduler; dissolution returns the graph to local mature
ownership. Borrowing integration, shared immutability proofs, and
compiler-proved barrier elimination remain separate required work.

Scoped arenas use typed `ArenaReference` tokens rather than managed references.
Their layouts distinguish scalar, same-arena, and managed-reference slots;
managed targets use precise internal roots that follow nursery relocation, while
arena edges never enter tracing. Bump allocation observes both arena capacity
and the global hard limit before mutation. Closing an arena releases all managed
roots and reclaims every arena object and byte as one deterministic operation.

Scheduler-local allocation records the owning scheduler in object ownership and
page metadata. Each scheduler retains an independent TLAB cursor and minor
request; local evacuation traces, relocates, and reclaims only that scheduler's
nursery, preserving other schedulers' tokens and pages. Direct local edges
between scheduler heaps are rejected before mutation. Parallel scheduler
execution and parallel local evacuation remain unfinished production work.

`RelocationRuntime` reports `RelocationConformance`, not production GC. It has
a moving nursery and card barrier but intentionally retains mature objects and
does not claim TLABs, parallel evacuation, concurrent mature marking, SATB,
adaptive sizing, or production pause behavior. See
[ADR 0039](../../../architecture/decisions/0039-relocating-nursery-root-and-backend-contract.md).

Each collector instance exposes saturating implementation telemetry for
successful allocations, actual collection cycles (including pressure-triggered
cycles), reclaimed objects, and scanned objects. The counters support tests and
benchmarks; they are not public Pop Lang reflection or a source-level API.

## Bootstrap benchmark

The custom benchmark records deterministic logical workload counters beside
timing data using the versioned `pop-runtime-benchmark-v1` schema:

```text
cargo bench -p pop-runtime-collector --bench bootstrap -- \
    --profile local-development --workload all --samples 5 --batches 32 \
    --items-per-batch 2048 --slots-per-object 2 --pressure-limit 256
```

The Stage-2 correctness comparator uses the same record schema with an explicit
`RelocationConformance` stage:

```text
cargo bench -p pop-runtime-collector --bench relocation -- \
    --profile local-development --samples 5 --batches 32 \
    --items-per-batch 2048
```

The bootstrap workload inventory covers tiny isolated objects, rooted reference chains,
managed-reference arrays, scoped pins, and automatic allocation pressure. It
measures the stable-handle Stage-1 collector only. It is useful for regression
baselines but is not evidence that the production moving nursery or concurrent
mature collector exists or meets its latency targets.
