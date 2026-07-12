# Runtime Implementation

The Rust runtime follows
[ADR 0038](../../architecture/decisions/0038-modular-portable-runtime-implementation.md).
These Cargo crates are host implementation boundaries, not Pop Lang Bubbles,
Packages, Modules, or public namespaces.

## Dependency direction

```text
                       pop-runtime-interface
                         ^              ^
                         |              |
            pop-runtime-collector   pop-runtime-native-abi
                         ^              ^
                          \            /
                           pop-runtime-native
```

The interface is the backend-neutral PLRI contract. The collector and native
ABI independently consume it. The native facade composes both. No dependency
points from the interface toward an implementation, from the collector toward
the native ABI, or from a backend-neutral compiler crate toward collector
internals.

## Choose the owning crate

- Change `pop-runtime-interface` for a previously accepted semantic PLRI value,
  operation, map, failure, or adapter contract. It must remain free of C symbols,
  platform state, and implementation storage.
- Change `pop-runtime-collector` for heap storage, reachability, roots, pins,
  collection scheduling, barriers, limits, or collection statistics. It must be
  usable without native linking or process-global state.
- Change `pop-runtime-native-abi` for the reviewed versioned C mapping of an
  accepted PLRI operation. Unsupported operations fail closed; do not add a
  fallback symbol or registry.
- Change `pop-runtime-native` for exported C functions, native process/global
  composition, UTF-8 process-entry adaptation, or target termination behavior.
  Heap semantics stay in the collector.

A new semantic operation or ABI contract requires architecture, negative and
cross-backend tests, then implementation. A collector optimization must preserve
precise maps, handles, failures, and observable behavior.

## Performance

Crate separation is compile-time organization. It must not introduce runtime
registration, string lookup, allocation, or virtual dispatch on native
allocation and barrier fast paths. Native code delegates to a concrete collector
and native symbols use a constant closed mapping.

Performance claims require the versioned GC benchmark suite with allocation
rate, live heap, roots, graph shape, cores, memory, CPU, throughput, and pause
percentiles recorded. Crate count and microbenchmarks alone are not evidence of
runtime performance.

The current `pop-runtime-benchmark-v1` bootstrap harness covers tiny objects,
rooted chains, managed arrays, scoped pins, and allocation pressure. Production
nursery, mature-heap, coroutine, server, and latency workloads remain required
before broader performance claims.

The relocation-conformance implementation is a correctness and comparison
stage, not a production claim. Benchmark records must identify
`BootstrapPreciseStopTheWorld`, `RelocationConformance`, or the future
production stage explicitly; results from different stages are not
interchangeable.

## Focused checks

The intended focused commands are:

```text
cargo test -p pop-runtime-interface
cargo test -p pop-runtime-collector
cargo test -p pop-runtime-native-abi
cargo test -p pop-runtime-native
```

Run architecture tests and applicable MIR-interpreter/LLVM differential tests
for changes that cross an ownership boundary.
