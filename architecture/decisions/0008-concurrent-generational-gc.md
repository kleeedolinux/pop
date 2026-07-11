# ADR 0008: Concurrent Generational Garbage Collector

- Status: accepted
- Date: 2026-07-10

## Context

Pop Lang needs fast allocation, low tail latency, native and VM support, precise
moving references, coroutine scanning, and predictable memory/CPU controls.
Choosing only “garbage collected” would leave compiler stack maps, barriers,
FFI, object headers, and runtime scheduling underspecified.

## Decision

The initial production collector uses thread-local allocation, a moving young
generation, a region-based mostly non-moving mature generation, parallel minor
collection, concurrent SATB mature marking, concurrent/lazy sweep, card marking,
precise stack/object maps, pacing, and bounded assists.

User finalizers and weak references are excluded from version one. Managed
references crossing FFI use handles or scoped pins. Runtime-private `TypeInfo`
and side metadata contain only facts required by GC/dispatch/casts.

Implementation proceeds through a simple precise collector before generations
and concurrency. Comparative speed claims require reproducible benchmarks.

## Consequences

- MIR/backends must represent safe points, barriers, precise roots, and handles.
- Young references can move, so raw foreign retention is forbidden.
- The runtime needs GC/scheduler handshakes, telemetry, memory control, and
  extensive stress/race testing.
- Most allocation is a thread-local pointer bump.
- Normal mature objects remain stable, reducing read-barrier and FFI complexity.

## Alternatives considered

### Reference counting

Rejected because atomic/recursive decrement cost, cycle handling, and pause
behavior conflict with the expected object/coroutine workloads.

### Non-generational concurrent mark-sweep only

Rejected as the target design because short-lived allocation is expected to be
common. It remains the useful bootstrap collector before the nursery exists.

### Fully moving concurrent collector

Deferred because read barriers, concurrent relocation, pinning, and LLVM
integration add substantial initial complexity. Mature fragmentation metrics
will determine whether selective compaction is later justified.

