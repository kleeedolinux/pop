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
protocol epochs without heap tracing in the handshake. Persistent named worker
threads receive immutable object snapshots through bounded per-worker queues,
scan exact object maps and remembered cards in parallel, and return
sequence-ordered results for collector-owned mutation. Refined cards become
precise young roots immediately inside the collecting safe point, where no
mutator store can invalidate the snapshot. Mature sweeping advances through the
ordered heap by a bounded cursor; the mark/sweep transition builds no heap-sized
unreachable-object inventory, and allocations during sweeping are live for that
cycle. It preserves snapshot edges, shades roots, pins, and new mature objects,
and defers nursery relocation while a major snapshot still
contains physical tokens. The implementation deliberately continues to report
`RelocationConformance`: epochs/workers are not yet integrated with native
scheduler transitions, and worker batches currently join each bounded collector
slice rather than tracing concurrently with mutator execution, so
`ProductionConcurrentGenerational` cannot yet be selected.

The same conformance runtime now records concrete Stage-2 allocation placement:
validated region/page/TLAB geometry, monomorphic page descriptors with precise
pointer layouts, scheduler-local Eden pointer bumps, separate mature/large/
pinned domains, survivor-copy placement, deterministic promotion, and immediate
pinned-space placement. A separate memory controller enforces a byte hard limit
before heap mutation, protects emergency and evacuation reserves, accounts
typed stack/code/metadata/native/arena/isolated usage, adapts the collection
target, performs bounded mature-cycle assists, returns empty logical pages, and
reports domain/debt/pressure/OOM telemetry. These logical descriptors validate
ownership and allocation transitions without exposing a raw address through
PLRI. Parallel per-scheduler TLAB ownership, virtual-memory reservation,
size-class reuse, adaptive work stealing, concurrent card refinement/lazy
sweeping, and measured production fast paths remain required before the
production profile.

This crate is reusable by native execution, the MIR interpreter, and a future
VM. It contains no C exports, native symbol mapping, platform process adapters,
linker policy, or process-global singleton. See
[ADR 0038](../../../architecture/decisions/0038-modular-portable-runtime-implementation.md).

The bootstrap implementation is divided into `heap`, `access`, `trace`, and
`adapter` modules. The `relocation` directory separately groups its heap,
collection, and adapter ownership; `generational` groups mature-cycle state,
mark/sweep work, barriers, page/TLAB allocation, memory control, coordination,
bounded workers, and its adapter. The allocation, coordination, memory, and
worker submodules separate public typed descriptors from mutable state.
These are static Rust partitions behind the same PLRI dependency, not runtime
plugins or dynamic dispatch.

The `generational::coordination` partition separates typed epoch/publication
vocabulary from its deterministic state machine. Detached and handle-only
mutators acknowledge automatically; managed mutators publish precise state;
bounded foreign transitions remain pending until they enter a safe state. This
is protocol infrastructure, not a claim that background collection or native
scheduler handshakes are complete.

The `generational::workers` partition owns persistent host threads, bounded
per-worker queues, immutable mark snapshots, deterministic result ordering,
telemetry, and joined shutdown. It performs parallel marking, remembered-card
refinement, and sweep dispatch only when explicitly configured; it does not
claim adaptive sizing, work stealing, mutator-concurrent tracing/refinement, or
concurrent heap mutation.

The ownership foundation currently implements scheduler-local and shared graph
publication. Isolated-region construction/transfer, scheduler-indexed local
heaps, scoped arenas, borrowing integration, and compiler-proved barrier
elimination remain separate required work; they are not simulated through the
shared publication path.

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
