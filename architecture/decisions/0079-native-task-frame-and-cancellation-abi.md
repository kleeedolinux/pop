# ADR 0079: Native Task-Frame and Cancellation ABI

- Status: accepted
- Date: 2026-07-14
- Depends on: ADR 0038, ADR 0039, ADR 0068, ADR 0077, and ADR 0078
- Supersedes: the ABI 1.11-only stable-facade clauses of ADR 0078

## Context

ADR 0068 fixes cold typed tasks, exact ownership, cooperative cancellation,
stackless coroutine frames, structured groups, and cancellation-masked async
cleanup. ADR 0077 binds ready and suspended native task frames to precise
collector-visible roots. The native ABI nevertheless exposes only scalar
`suspend`/`resume` placeholders and a cancellation query that retains no
request. LLVM therefore cannot construct or schedule the verified task frames
already present in canonical MIR.

The task transition must not weaken ABI 1 stable managed-reference tokens or
pretend that the ABI 2 moving-root proof is complete. It must also keep an
immutable `CancelToken` distinct from the authority that requests
cancellation. Treating any nonzero token as cancellation authority would make
capability boundaries forgeable and would disagree with ADR 0068.

## Decision

### Coexisting closed descriptors

Native ABI 1.12 extends ABI 1.11 with the task-frame and cancellation entries
defined here. ABI 1.11 remains a supported immutable descriptor: its existing
symbols and meanings do not change. `pop_rt_supports_abi(1, 11)` and
`pop_rt_supports_abi(1, 12)` may therefore both succeed in an ABI 1.12 facade.

ABI 1.12 retains stable managed-reference tokens and cannot satisfy the
`ProductionGenerational` profile. ABI 2.0 remains the distinct writable-root
descriptor from ADR 0078; adding task entries does not authorize a facade to
report ABI 2 support. A later complete production descriptor must combine
these task entries with the ABI 2 moving-root postconditions explicitly.

### Cancellation authority

The first-release source surface uses direct namespace functions rather than a
factory class or ambient current-task object:

```luau
local source: Task.CancelSource = Task.cancellationSource()
local cancel: CancelToken = Task.cancelToken(source)
Task.cancel(source)
```

`Task.cancellationSource()` is the allocation point. `Task.cancelToken(source)`
copies the immutable observation token, and `Task.cancel(source)` requests
cancellation idempotently. `Task.Group` remains the opaque lexical owner passed
only to the body of `Task.group`; `Task.start(group, task)` transfers a cold
task and returns that same typed `Task<T>` handle. These are namespace
functions, not static utility-class members, reflective constructors, or
implicit context lookup.

The native facade supplies fixed entries to create a `Task.CancelSource`, copy
its immutable token, release each retained handle, request through the source,
and query through the token:

```text
pop_rt_cancel_source_create() -> source
pop_rt_cancel_source_token(source) -> token
pop_rt_cancel_source_release(source) -> status
pop_rt_cancel_token_release(token) -> status
pop_rt_task_cancel(source) -> status
pop_rt_task_cancellation_requested(token) -> status
```

Zero is the invalid handle. Source and token handles are distinct typed runtime
identities even when the physical ABI carries each as `u64`. A token never
grants request authority. Requests are persistent and idempotent; releasing a
source does not revoke already copied tokens or clear an observed request.

### Opaque compiler frame boundary

LLVM creates an opaque native frame from compiler-proven scalar slots and an
exact `RootSlot` map. It accesses those slots only through fixed indexed frame
operations. Before returning a nonterminal poll, generated code installs the
next MIR resume-state identity and that state's exact live root slots. The
scheduler publishes this frame before the task becomes ready or suspended and
restores relocated values before invoking generated code again.

The generated poll callback has the closed logical signature:

```text
poll(task, frame, cancellationRequested) -> TaskPollStatus
```

Cold creation and structured wrapping carry the compiler-proven completion
representation explicitly:

```text
pop_rt_task_create(frame, poll, token, completionIsManaged) -> task
pop_rt_task_group_wrap(group, body, completionIsManaged) -> task
```

`completionIsManaged` is the closed zero-or-one projection of the verified MIR
completion type. The returned task is a precisely mapped control object whose
first slot retains the terminal completion when that type is managed and whose
second slot retains the optional cancellation token. Retaining the task
therefore retains and relocates both edges without an untraced side-table value.
Before admission, the runtime retains every exact managed root in the cold
frame; scheduler admission overlaps those cold roots with the scheduler's
ready-frame publication before releasing them. Unreachable cold and terminal
control records are weak side-table entries pruned after collection, not
source-visible finalizers.

`TaskPollStatus` is a native-ABI closed enum for ready, pending, completed,
cancelled, and panicked. It is not source-visible reflection. The callback
contains no backend handle in HIR or MIR; it is LLVM-private code derived only
from verified canonical MIR.

The native task entries cover frame creation and release, indexed slot
load/store, exact live-map replacement, cold task creation, direct-await or
group start, completion observation, group closure/join, cancellation request,
and terminal release. Every failure uses a closed status or zero handle and is
failure-atomic. There is no string-named callback, global source-visible task
lookup, detached start, untyped resume value, or dynamic fallback.

Canonical typed effects include `Synchronizes` for ownership transfer,
cancellation requests, and scheduler/group coordination. It is distinct from
`Suspends`: a synchronization operation need not park the current task, while
an await that may park records both the scheduler requirement and `Suspends`.
HIR and MIR retain this closed effect; a backend maps it to the coroutine
scheduler contract rather than introducing a backend-only operation.

### Await and structured group transitions

Cold task creation retains its compiler frame but does not enter a ready queue.
Direct await atomically establishes `DirectAwait` ownership and start-once
state; `Task.start(group, task)` atomically establishes the exact group owner.
Repeated start fails without replacing the first owner.
Scheduler admission rejection restores the still-unpolled frame, owner, group
membership, and cancellation binding before returning failure.

An incomplete await records the exact waiting task, publishes the caller's
current live frame, and returns `Pending` from its generated poll. Terminal
completion wakes that waiter. A completed task returns its retained typed
completion without rerunning its body. Cancellation and panic use distinct
terminal statuses and enter the canonical MIR cancellation or unwind edge.

A closing group rejects new children, requests cancellation of every
unfinished child through its owned cancellation authority, and retains the
original body/child exit until all children are terminal and joined. Child
panic requests sibling cancellation before propagation. No group completion
or release can leave a child alive. Joining releases each group-owned terminal
task record; the group wrapper releases its terminal body record after copying
the typed completion into its own precisely mapped frame.

### Cancellation masking

LLVM lowers each MIR `Suspend.cancellation_mode` exactly. `Observe` checks the
explicit request before the suspension and selects the MIR cancellation edge.
`Masked` records but does not select that edge while async cleanup is active.
The request remains pending and becomes observable at the next unmasked
cancellation point. The native scheduler never destroys the frame or skips
cleanup asynchronously.

## Consequences

- Native ABI 1.12 can execute compiler-created cold task frames without
  changing ABI 1 stable-root semantics.
- Cancellation authority cannot be forged by copying a `CancelToken`.
- Ready and suspended LLVM frames use the same precise root lifecycle already
  required of native scheduler tasks.
- MIR-interpreter and LLVM execution share task ownership, terminal outcomes,
  cancellation modes, and cleanup edges without sharing backend objects.
- ABI 2 production capability remains gated on writable-root relocation proof.

## Alternatives considered

### Keep scalar task handles as their own completion

Rejected because no cold frame, owner, suspension, cancellation request,
continuation, or precise ready/suspended roots exist.

### Let any token request cancellation

Rejected because immutable observation tokens would become authority-bearing
and forgeable, contrary to ADR 0068.

### Store native callback names and resolve them at runtime

Rejected because string-based function resolution is an operational dynamic
escape hatch. LLVM emits fixed typed callback addresses.

### Root the union of every value ever live in the coroutine

Rejected because it is not the exact compiler-proven live frame and can retain
dead object graphs indefinitely. Each resume state installs its precise map.

### Advertise ABI 2 when task execution works

Rejected because task scheduling does not prove writable-root relocation or
post-safe-point SSA replacement.

## Required conformance tests

- ABI 1.11 and 1.12 coexist without changing an ABI 1.11 symbol;
- source and token handles are distinct, token-as-source fails, and a request
  persists idempotently until token release;
- cancellation-source construction, token copying, and request calls have the
  exact direct namespace signatures and reject wrong nominal arguments;
- cold creation performs no poll and direct/group start succeeds exactly once;
- ready and suspended compiler frames publish exact roots, accept relocation,
  and install every returned slot before the next poll;
- await completion, cancellation, and panic select distinct verified MIR edges;
- completed repeated await returns the retained value without rerunning;
- group exit cancels and joins every unfinished child, and child panic cancels
  siblings before propagation;
- masked cleanup awaits cannot select cancellation while the request remains
  pending for the next unmasked point;
- MIR interpreter and LLVM produce identical results, side effects, cleanup
  order, and terminal outcome before and after MIR/LLVM optimization; and
- C and unsupported targets reject coroutine runtime requirements before code
  emission without fallback.

## Documents/components affected

Runtime and ABI architecture, native ABI vocabulary and facade, scheduler task
adapters, LLVM private lowering, foundational task APIs, backend capability
validation, conformance tests, closed design decisions, and roadmap.
