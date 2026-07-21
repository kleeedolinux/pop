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

ABI 1.13 adds balanced foreign transitions. Blocking calls retain every live
root in runtime-owned handles while the scheduler mutator is `HandlesOnly`;
reviewed nonblocking calls use `BoundedForeign`. Both modes require the current
managed scheduler binding, preserve an exact writable root shape, and consume
one thread-local LIFO transition token on leave.

ABI 1.14 adds balanced managed-thread attachment for generated program entry
and callback adapters. Attachment registers one exact scheduler mutator,
enters managed state, and returns explicit thread-bound authority. Detach is
rejected until every foreign transition is closed, then clears and unregisters
the same binding.

ABI 1.18 adds callback registration behind an opaque 64-bit context token.
Entry validates one compile-time callback site, thread/scheduler policy, and
serialized non-reentrant state before restoring managed execution; leave
restores the exact foreign state or detaches an entry-created binding. Close
invalidates the context before releasing the rooted managed environment.

ABI 1.19 adds the two fixed-width codec event operations. Generated adapters
select exact schemas statically; the native facade carries only sealed
writer/reader capability events and never parses `.popc`, resolves runtime
names, or maintains an adapter registry.

Heap storage, reachability, roots, pins, and collection policy remain in
`pop-runtime-collector`; symbol/version vocabulary remains in
`pop-runtime-native-abi`. See
[ADR 0038](../../../architecture/decisions/0038-modular-portable-runtime-implementation.md).
The native collector transition is specified by
[ADR 0070](../../../architecture/decisions/0070-native-stable-generational-transition.md).
Atomic initialized publication is specified by
[ADR 0072](../../../architecture/decisions/0072-atomic-initialized-object-allocation.md).

The facade is divided into `identity`, `allocation`, `binding`, `storage`,
`text`, `roots`, `foreign`, `ffi_callback`, `failure`, `scheduler`, and private
`state` modules.
The scheduler provides the bounded synchronized M:N correctness implementation,
deterministic
per-dispatch work budgets and record/replay, typed collector-transition hooks,
bounded scheduler-work-unit ready and driver-delivery percentiles, and a
separate bounded blocking pool plus bounded host/virtual timer and
external-event delivery. Native shutdown records one bounded blocking-pool
drain/join delay and remains idempotent under subsequent drop cleanup. These
contracts are specified by the
[scheduler runtime design](../../../architecture/23.1-scheduler-runtime-implementation.md).
This keeps ABI exports grouped by the runtime service they adapt while
retaining one static library and one native ABI.

[ADR 0077](../../../architecture/decisions/0077-scheduler-mutator-and-task-root-binding.md)
defines the scheduler/collector binding. Each normal worker now owns one
detached mutator registration for its lifetime, enters managed state only while
polling a task, carries an exact thread-local scheduler/mutator binding through
serialized native ABI operations, acknowledges active collection epochs at
managed safe points, and unregisters on shutdown. Every ready or suspended task
frame owns one collector-visible precise root container. ABI 1 remains the
stable-token serialized correctness stage; moving native execution still waits
for the ABI 2 backend reload proof. The staged
`pop_rt_gc_safe_point_v2` entry performs failure-atomic writable slot
installation, but this stable facade deliberately rejects ABI 2 capability
negotiation.

The checksum-validated synchronized-reference benchmark is available with:

```text
cargo bench -p pop-runtime-native --bench scheduler -- \
  --profile local-declared --workload all --workers standard
```

Its `pop-scheduler-benchmark-v3` records label the target, scheduler stage,
workload, worker profile, logical work, initial dispatch latency scope, queue
depths and high-water marks, steal outcomes, worker lifecycle, bounded
scheduler-work delay percentiles, and labelled operating-system memory and
context-switch observations. The default run is local optimization evidence,
not a portable performance claim; scale and GC-coupled evidence must use the
explicit profiles below.

The scale and collector profiles are explicit rather than implied by the
default run:

```text
cargo bench -p pop-runtime-native --bench scheduler -- \
  --samples 1 --tasks 1000000 --polls-per-task 1 --workers available \
  --workload suspended_frames --profile million-suspended-minimal-frames

cargo bench -p pop-runtime-native --bench scheduler -- \
  --samples 5 --tasks 8192 --polls-per-task 16 --workers available \
  --workload scheduler_gc_interaction --profile scheduler-gc-interaction
```

`local_wake`, `foreign_wake`, `ping_pong`, `steal_storm`, and
`continuous_event_fairness` select the corresponding typed latency/fairness
workloads. `task_control` includes the park/unpark path, while
`burst_injection` is the global-injection profile.
