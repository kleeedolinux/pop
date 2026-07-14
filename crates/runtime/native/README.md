# Native Runtime Facade

`pop-runtime-native` composes the portable collector with the versioned native
ABI. It owns exported C functions, the process-global synchronized stable-token
generational instance, UTF-8 and process-entry adaptation, and native
trap/unwind termination. ABI 1 native allocations remain non-moving while
using incremental SATB mature marking and bounded sweeping; moving nursery and
evacuation require the future ABI 2 writable-root contract.

ABI 1.11 adds atomic initialized-object allocation: the facade validates the
complete precise map and every managed initializer before delegating one
failure-atomic publication to the stable collector. Ordinary post-publication
mutation continues through checked scalar or reference-store paths.

Heap storage, reachability, roots, pins, and collection policy remain in
`pop-runtime-collector`; symbol/version vocabulary remains in
`pop-runtime-native-abi`. See
[ADR 0038](../../../architecture/decisions/0038-modular-portable-runtime-implementation.md).
The native collector transition is specified by
[ADR 0070](../../../architecture/decisions/0070-native-stable-generational-transition.md).
Atomic initialized publication is specified by
[ADR 0071](../../../architecture/decisions/0071-atomic-initialized-object-allocation.md).

The facade is divided into `identity`, `allocation`, `storage`, `text`, `roots`,
`failure`, `scheduler`, and private `state` modules. The scheduler provides the
bounded synchronized M:N correctness implementation, deterministic
per-dispatch work budgets and record/replay, typed collector-transition hooks,
bounded scheduler-work-unit ready-delay percentiles, and a separate bounded
blocking pool plus bounded host/virtual timer and external-event delivery
specified by the
[scheduler runtime design](../../../architecture/23.1-scheduler-runtime-implementation.md).
This keeps ABI exports grouped by the runtime service they adapt while
retaining one static library and one native ABI.

[ADR 0072](../../../architecture/decisions/0072-scheduler-mutator-and-task-root-binding.md)
defines the scheduler/collector binding. Each normal worker now owns one
detached mutator registration for its lifetime, enters managed state only while
polling a task, carries an exact thread-local scheduler/mutator binding through
serialized native ABI operations, acknowledges active collection epochs at
managed safe points, and unregisters on shutdown. Every ready or suspended task
frame owns one collector-visible precise root container. ABI 1 remains the
stable-token serialized correctness stage; moving native execution still waits
for the ABI 2 writable-root contract and its backend reload proof.

The checksum-validated synchronized-reference benchmark is available with:

```text
cargo bench -p pop-runtime-native --bench scheduler -- \
  --profile local-declared --workload all --workers standard
```

Its `pop-scheduler-benchmark-v2` records label the target, scheduler stage,
workload, worker profile, logical work, initial dispatch latency scope, queue
depths and high-water marks, steal outcomes, worker lifecycle, and other typed
telemetry. The default run is local optimization evidence, not a portable
performance claim or a substitute for the pending GC-coupled and operating-
system resource profiles.
