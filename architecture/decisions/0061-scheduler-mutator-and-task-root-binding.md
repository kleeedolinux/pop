# ADR 0061: Scheduler Mutator and Task-Root Binding

- Status: accepted
- Date: 2026-07-14
- Depends on: ADR 0008, ADR 0022, ADR 0038, ADR 0039, ADR 0057, and ADR 0059
- Supersedes: none

## Context

The native scheduler now has bounded logical schedulers, workers, ready queues,
work stealing, deterministic execution, blocking isolation, event delivery,
and typed transition hooks. The collector separately has scheduler-indexed
allocation, typed mutator registration, bounded epoch handshakes, precise
mutable root publications, and scheduler-local ownership.

Those foundations do not yet form one safe runtime contract:

- `GenerationalRuntime` has one mutable selected scheduler, which cannot
  represent concurrent worker bindings unless selection and each operation are
  kept atomic;
- transition hooks report worker and task events but do not retain the precise
  managed roots of queued or suspended task frames;
- a ready frame is just as live as a suspended frame while it waits in a queue,
  so rooting only explicit suspension would still permit collection of its
  state;
- a moving collector can replace every physical managed token, so a frame must
  install updated `RootSlot` values before its next poll;
- a worker registered as managed cannot park or wait indefinitely without
  acknowledging an active collector epoch; and
- scheduler shutdown and failed admission must release retained frame roots
  exactly once.

Treating a Rust task object, queue entry, raw token vector, process-global
selected scheduler, or stable ABI 1 token accident as an implicit root would
contradict the precise GC and backend contracts. The integration must be typed,
failure-atomic, and usable by the MIR interpreter, LLVM, and a future VM without
putting native scheduler structures in HIR or MIR.

## Decision

### Shared runtime identities and ownership

`SchedulerId` is a backend-neutral PLRI identity. Its canonical definition
belongs to `pop-runtime-interface`; the collector may re-export it during the
implementation transition, but it does not own the semantic identity.

Mutator registrations and retained task-root containers use separate opaque
typed identities. A mutator identity names one registered worker execution
context. A task-root identity names one collector-owned precise frame-root
container. Neither identity is a source value, object identity, pointer, string
lookup key, reflection handle, or authority to inspect a task.

Every retained task is owned by exactly one logical scheduler at a time. Queue
movement within that scheduler does not change ownership. Cross-scheduler
migration commits only after the collector accepts the exact root-container and
local-graph transition; refusal leaves both task ownership and queue placement
unchanged.

### Precise task-frame root lifecycle

Every scheduler task has a compiler/runtime-known frame descriptor with two
typed operations:

1. publish one canonical mutable `RootPublication` for its current frame; and
2. restore an updated publication into the exact frame slots by `RootSlot`.

There is no default empty implementation. A trusted runtime or test task that
provably contains no managed references uses an explicit empty-frame
publication with a real safe-point/frame identity. Publication and restoration
cannot enumerate fields dynamically, parse metadata names, conservatively scan
Rust memory, or accept a differently shaped stack map.

The root lifecycle is:

1. task admission publishes and retains the initial frame roots before any
   ready-queue entry becomes visible;
2. dispatch removes the retained container, obtains its possibly relocated
   publication, restores every changed frame slot, and only then polls the task;
3. while the task is running, ordinary compiler stack maps and safe points own
   its active roots;
4. a nonterminal `Ready` or `Pending` poll publishes a new frame-root container
   before the task becomes visible as queued or suspended;
5. terminal completion, cancellation, or panic retains no task-frame root
   container; and
6. shutdown, failed admission, and abandoned internal tasks release every
   retained container exactly once.

The collector validates the publication before changing task state. A failure
cannot leave an unrooted visible queue entry or discard the last valid root
container. Restored roots must be installed before cancellation cleanup or user
code can observe the frame again.

The correctness implementation may represent a frame-root container as a
canonical stack map plus one strong runtime handle per present root slot. Those
handles are collector roots and receive relocation updates. This representation
is not a public ABI promise; a later optimized collector may scan heap-owned
frame containers directly when it preserves the same exact slots, ownership,
and failure behavior.

### Worker mutators and epoch participation

One normal scheduler worker has one registration for its lifetime:

- worker start registers it as `Detached` for its exact logical scheduler;
- task dispatch restores the task roots, binds the worker and scheduler in a
  non-escaping native execution context, and transitions it to `Managed`;
- every poll return publishes nonterminal frame roots when needed, clears the
  managed execution binding, and transitions the worker back to `Detached`;
- parking remains detached and therefore acknowledges an epoch without a stack
  publication;
- re-entry from bounded foreign code uses the existing explicit managed/handle-
  only/foreign states; and
- worker stop unregisters it after no managed task or native binding remains.

The scheduler loop itself contains no hidden managed roots. A worker never
remains `Managed` while waiting on a queue, condition variable, event driver, or
blocking operation.

Every native ABI operation reached from managed task code resolves the current
typed execution binding and selects that scheduler while holding the current
serialized native-runtime lock. Scheduler selection and the collector operation
are one critical section, so another worker cannot change the allocation owner
between them. A thread-local cell may transport this non-escaping host binding;
it is not semantic global state, an implicit Pop global, or permission for
unregistered threads to enter managed code.

The serialized stable-token composition is a correctness stage, not the final
parallel allocation fast path. Removing the process-global runtime lock later
requires collector operations or allocator contexts that carry the same
explicit mutator/scheduler identity; it cannot restore one shared mutable
selected scheduler.

### Collecting safe points and handshakes

The runtime exposes one mutator-aware safe-point operation. It atomically:

1. validates the caller's registration, scheduler binding, stack map, and roots;
2. begins a requested epoch when necessary;
3. acknowledges the active epoch exactly once for that mutator with the current
   roots and allocator/barrier publication;
4. performs only the bounded assist or collection work authorized for that safe
   point; and
5. writes every relocated root token back before returning.

An epoch that begins during the call cannot miss the initiating mutator. A
duplicate poll of an already acknowledged epoch is a successful no-op for the
handshake portion, not a second publication and not an `AlreadyAcknowledged`
failure exposed to generated code. Stale or foreign mutator identities fail
closed.

Queued and suspended task-root containers participate independently of worker
acknowledgements. Major and scheduler-local minor root discovery includes every
retained container. A task need not resume to be scanned or updated.

### Transition and queue publication order

Runtime transitions that can fail occur before observable scheduler state is
committed. For admission, wake, cancellation, poll completion, migration, and
shutdown, the required order is:

```text
validate bounds and current task state
→ validate/commit collector root or ownership transition
→ update task state and ready/activity accounting
→ publish the queue entry or terminal state
→ notify workers or waiters
```

No worker may consume a queue entry before its ready/activity accounting and
root-container ownership are visible. Rollback restores the previous complete
state; it never invents a root publication, duplicates a task, or reports an
architecture/runtime incident as an ordinary task panic.

### Runtime profiles and backend boundary

ADR 0059's `NativeStableGenerationalConformance` composition implements this
binding while keeping every ABI 1 allocation non-moving. It must still retain
precise queued/suspended roots, use exact scheduler ownership, and participate
in epochs; stable physical tokens do not waive those requirements.

Moving nursery execution remains gated by ADR 0039. LLVM enables it only with
ABI major 2 writable roots and post-safe-point reload proof. The MIR interpreter
and future VM use the same task-root lifecycle directly with their typed frame
slots. The experimental C backend continues to reject tasks and coroutine
frames.

### Failures and observability

Task panic remains an isolated task terminal state. Collector registration,
root-shape, relocation, epoch, ownership, or restoration failure is a typed
runtime/architecture failure and stops the affected scheduler runtime; it is
not converted into cancellation, task panic, an empty root set, or migration
success.

Telemetry records mutator registration/unregistration, managed/detached
transitions, epoch polls/acknowledgements, retained task-root containers and
slots, root restoration, migration acceptance/refusal, and cleanup failures.
It exposes counts and timing facts, never frame values or heap contents.

## Consequences

- Ready and suspended tasks remain precise roots without depending on native
  thread stacks or accidental stable handles.
- Scheduler workers participate in collection epochs without remaining managed
  while parked.
- Native scheduler ownership is race-free even during the serialized ABI 1
  correctness stage.
- Relocation updates flow through `RootSlot` restoration before a task resumes.
- The scheduler task implementation contract becomes stricter: every task must
  explicitly publish and restore its frame roots.
- Initial handle-per-slot root containers may cost more than direct frame
  scanning, but provide a bounded correctness baseline that later optimization
  must match differentially.
- Full production moving execution still depends on ABI 2 and backend reload
  proof; this ADR does not relabel ABI 1 as production.

## Alternatives considered

### Root only explicitly suspended tasks

Rejected because a ready task waiting in a local or injection queue still owns
a live frame and may be stolen while collection proceeds.

### Treat the Rust task object or queue as a conservative root

Rejected because Rust object layout is not a compiler-proven Pop frame map and
conservative scanning violates relocation, ownership, and liveness contracts.

### Keep one process-global selected scheduler

Rejected because separate workers can interleave selection and allocation,
placing objects in the wrong scheduler-local heap. Serialization is correct
only when binding selection and the operation are one critical section.

### Give every task a native thread stack

Rejected because it abandons the accepted lightweight M:N model and its high
suspended-task-count target.

### Keep every frame object alive through ordinary public root handles forever

Rejected because it loses exact frame-slot restoration and leaks task lifetime
into a public/manual handle discipline. Strong handles may implement the private
container, but the scheduler owns their bounded lifecycle.

### Delay all root integration until coroutine syntax is implemented

Rejected because scheduler admission, migration, collection, and shutdown
would otherwise stabilize around an unscannable task-object contract that the
compiler could not safely adopt later.

## Required conformance tests

- canonical `SchedulerId` lives in PLRI and collector/native components share
  the same typed identity;
- worker start/register, dispatch/manage, poll-finish/detach, park, unpark, and
  stop/unregister sequences are exact and remain bounded under failure;
- two workers cannot allocate under one another's scheduler ownership when ABI
  calls interleave;
- admission fails before queue publication when initial frame roots are invalid;
- ready and suspended frame roots keep their complete transitive graphs alive
  without resuming the task;
- forced minor and major collection update retained frame slots and restoration
  installs the new tokens before the next poll;
- empty trusted host frames require an explicit exact empty publication;
- wrong stack-map shape, stale root token, duplicate restore, unknown root
  handle, and restoration mismatch fail closed without losing the last valid
  container;
- wake/cancellation races cannot expose a queue entry before root ownership and
  ready/activity accounting;
- migration acceptance moves the exact root container/ownership once, while
  refusal leaves source ownership and queue placement unchanged;
- shutdown with ready, suspended, cancelled, panicked, and event-waiting tasks
  releases every retained root container once;
- an epoch begun by a mutator safe point includes that mutator, detached workers
  acknowledge automatically, duplicate polls do not duplicate publication, and
  stale identities fail;
- deterministic and native schedulers preserve the same root lifecycle and
  task-state transitions;
- MIR interpreter, LLVM stable-token execution, and future ABI 2 relocation
  preserve the same root values and task outcomes; and
- no conservative scan, implicit empty roots, raw managed-pointer escape,
  runtime string lookup, backend-specific HIR/MIR, or moving-ABI claim enters
  the implementation.

## Documents/components affected

PLRI scheduler/root identities, scheduler runtime implementation, native task
contract, stable-generational facade, generational mutator coordination,
relocation roots, native ABI entry binding, MIR coroutine-frame lowering and
interpreter, LLVM safe points, future VM frames, architecture tests, scheduler/
GC stress suites, telemetry, benchmark profiles, closed decisions, and roadmap.
