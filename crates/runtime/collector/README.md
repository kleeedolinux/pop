# Portable Runtime Collector

`pop-runtime-collector` owns heap storage, precise tracing, roots, pins,
collection requests, limits, and statistics. `BootstrapRuntime` implements the
Stage-1 stable-handle collector. `RelocationRuntime` implements the first
single-mutator Stage-2 conformance slice: it copies live young objects, updates
typed roots, object edges, strong handles, and pins, invalidates old tokens,
promotes deterministically, and maintains remembered cards.

`GenerationalRuntime` composes that moving nursery with a modular incremental
mature-heap conformance slice. Its `generational` modules separate PLRI
adaptation, SATB/publication barriers, cycle state, and bounded mark/sweep work.
It preserves snapshot edges, shades roots, pins, and new mature objects, and
defers nursery relocation while a major snapshot still contains physical
tokens. The implementation deliberately continues to report
`RelocationConformance`: mature tracing is cooperative and incremental, not yet
the background-worker, page/TLAB, paced production collector required before
`ProductionConcurrentGenerational` may be selected.

The same conformance runtime now records concrete Stage-2 allocation placement:
validated region/page/TLAB geometry, monomorphic page descriptors with precise
pointer layouts, scheduler-local Eden pointer bumps, separate mature/large/
pinned domains, survivor-copy placement, deterministic promotion, and immediate
pinned-space placement. These logical descriptors validate ownership and
allocation transitions without exposing a raw address through PLRI. Parallel
per-scheduler TLAB ownership, virtual-memory reservation, size-class reuse, and
measured production fast paths remain required before the production profile.

This crate is reusable by native execution, the MIR interpreter, and a future
VM. It contains no C exports, native symbol mapping, platform process adapters,
linker policy, or process-global singleton. See
[ADR 0038](../../../architecture/decisions/0038-modular-portable-runtime-implementation.md).

The bootstrap implementation is divided into `heap`, `access`, `trace`, and
`adapter` modules. The `relocation` directory separately groups its heap,
collection, and adapter ownership; `generational` groups mature-cycle state,
mark/sweep work, barriers, page/TLAB allocation, and its adapter. The allocation
submodules separate public typed descriptors from mutable placement state.
These are static Rust partitions behind the same PLRI dependency, not runtime
plugins or dynamic dispatch.

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
