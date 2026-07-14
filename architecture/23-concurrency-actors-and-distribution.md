# Concurrency, Actors, and Distribution

## Status and authority

This document integrates
[ADR 0068](./decisions/0068-typed-async-tasks-actors-and-distribution.md).
It defines Pop Lang's accepted concurrency, isolation, supervision, and
distribution architecture. Exact library signatures may grow through normal
API review, but they cannot weaken the semantic boundaries here.

## Product contract

Pop Lang combines four properties:

- Luau-shaped `async function`, prefix `await`, and `end` blocks;
- lightweight M:N tasks and typed channels suitable for high-concurrency
  servers;
- Erlang/Elixir-inspired actor isolation and supervision with exact static
  mailbox types; and
- explicit distributed actor endpoints with typed schemas, security
  capabilities, backpressure, and partial-failure results.

This is not one universal concurrency object. The fixed model is:

```text
async function call
    -> cold Task<T>
        -> direct await, or Task.Group concurrent ownership
            -> typed Channel<T> coordination where sharing is intended

Actor.Ref<TMessage>
    -> bounded local mailbox
        -> copied message in private actor ownership
            -> one sequential actor entry + structured child tasks
                -> Actor.Supervisor failure/restart policy

Cluster.Actor<TMessage>
    -> explicit authenticated transport
        -> bounded schema encoding/decoding
            -> remote actor mailbox admission or typed delivery uncertainty
```

Tasks, actors, operating-system processes, Modules, Bubbles, Packages,
Workspaces, and cluster nodes remain distinct concepts. Implementation reuse
does not merge their semantics or names.

## Async source model

An async function declares its completion value. Calling it creates a cold
`Task<T>`:

```luau
async function fetch(uri: Uri, cancel: CancelToken): Result<Page, Http.Error>
    return await Http.get(uri, cancel)
end

local task: Task<Result<Page, Http.Error>> = fetch(uri, cancel)
local page = try await task
```

The call evaluates arguments once and allocates the task/frame, but the body
does not begin until direct `await` or `Task.start(group, task)` takes
ownership. Async and synchronous function types never convert implicitly.

`await` accepts exactly `Task<T>`, is valid only in async code, and yields
exactly `T`. It is a suspension, cancellation, scheduling, flow-fact, and GC
safe point. A completed task can be awaited again without rerunning it. Panic,
cancellation, and expected `Result` failure remain separate paths.

Async state machines are the first-release coroutine model. Pop Lang has no
untyped `resume`/`yield` carrier, status-string protocol, dynamic generator, or
arbitrary multiple-value exchange. A typed generator or async iterator needs a
separate decision.

## Structured task ownership

Every started task has an owner. `Task.group` is the ordinary concurrent
ownership boundary:

```luau
async function fetchPair(cancel: CancelToken): Result<(Page, Page), Http.Error>
    return await Task.group(cancel,
        async function(group: Task.Group): Result<(Page, Page), Http.Error>
            local left = Task.start(group, Http.get(leftUri, cancel))
            local right = Task.start(group, Http.get(rightUri, cancel))
            return Result.Ok(await left, await right)
        end)
end
```

Leaving the group by completion, expected failure, cancellation, or panic
cancels unfinished children and joins all cleanup. A child panic cancels its
siblings before propagating. No child outlives the group.

Detached work is not a default language operation. A long-lived host or
application supervisor can own work only through an explicit capability and
shutdown contract.

## Cancellation and cleanup

`CancelToken` is explicit and immutable. A `Task.CancelSource` or owning group
requests cancellation. Observation is cooperative at await, channel/actor
operations, cancellation-aware I/O, scheduler polls, and bounded-work
backedges. Cancellation never destroys a frame asynchronously.

Ordinary `defer` cannot suspend. `async defer` registers cancellation-masked,
last-in, first-out cleanup:

```luau
local stream = try await File.open(path, cancel)
async defer
    await File.close(stream)
end
```

Async cleanup runs on fallthrough, return, `try` propagation, loop control,
panic, and cancellation. It cannot contain a control exit, prefix `try`, or
another cleanup declaration. The original cancellation remains pending until
every registered cleanup completes.

## Scheduler contract

Tasks are user-space stackless coroutines scheduled M:N over a bounded worker
set. They are never specified as one native thread per task. Ready work may
migrate or be stolen only when GC publication and ownership invariants permit
it. A suspended frame retains exactly its live typed locals, captures, cleanup
state, resume state, and precise root map.

Semantic scheduling opportunities include:

- await, task yield, channel/actor operations, and timers;
- allocation slow paths and runtime/FFI transitions;
- loop backedges after a deterministic bounded amount of work; and
- compiler-inserted polls in long straight-line regions.

Independent ready tasks have no promised wall-clock order. The test scheduler
records and replays every allowed choice. Production policy, worker count,
queue layout, and work stealing remain private runtime details.

The scale target is thousands or millions of suspended tasks with memory
proportional to live frames. It is a benchmark and regression target, not an
unbounded allocation guarantee. Runtime memory, task, queue, and blocking-pool
limits remain explicit.

An operation that blocks an OS worker has the distinct `Blocks` effect. Async
libraries use nonblocking host adapters or an explicit bounded blocking pool;
they cannot hide a blocking syscall behind `async` spelling.

## Channels and selection

`Channel<T>` is exactly typed. Directional sender/receiver endpoints,
capacity, close behavior, allocation, and cancellation are part of the public
contract. Bounded channels are the default for streams and pipelines.
Unbounded channels require an explicit advanced constructor and memory-limit
documentation.

Send and receive may suspend and apply backpressure. `Task.select` uses a
closed typed set of task, channel, or timer cases. It never returns an untyped
tag/value bag. When several cases are ready, the scheduler chooses unless the
caller supplies an explicit priority policy; tests can replay the choice.

Channels do not imply private heaps, panic containment, supervision, durable
messaging, or network transport. Those belong to actors, supervisors,
`Message`, and `Cluster` respectively.

## Local actor model

An actor owns exactly one:

- incarnation-scoped `Actor.Ref<TMessage>`;
- bounded FIFO mailbox of `TMessage`;
- private mutable state/resource domain;
- structured child-task group; and
- normal/cancel/panic exit boundary.

No new declaration syntax is needed. An ordinary async entry receives a
non-escapable inbox and explicit cancellation:

```luau
public union CounterMessage
    Increment(amount: Int)
    Read(reply: Actor.Reply<Int>)
end

private async function runCounter(
    inbox: Actor.Inbox<CounterMessage>,
    cancel: CancelToken,
)
    local count = 0
    while local message = await Actor.receive(inbox, cancel) do
        match message
        when CounterMessage.Increment(amount) then
            count += amount
        when CounterMessage.Read(reply) then
            Actor.reply(reply, count)
        end
    end
end

local counter: Actor.Ref<CounterMessage> =
    try await Actor.start(supervisor, { capacity = 256 }, runCounter)
```

Only the actor entry dequeues messages, one at a time. Suspending inside one
handler does not reenter actor state; later messages remain queued. The initial
mailbox is whole-queue FIFO rather than selective receive. Messages accepted
from one sender retain order; messages from different senders may interleave.

`Actor.send` suspends when the bounded mailbox is full and returns a typed
result. Completion means mailbox admission, not handler completion.
`Actor.trySend` reports full, closed, or stale incarnation without suspending.
`Actor.Reply<T>` is a typed single-use response capability.

## Message and capture safety

The compiler proves a recursive actor-message-safe property. It is not a
marker interface, user overload, retained reflection query, or runtime test.

The first accepted actor message graph contains:

- primitive values, `String`, and enums;
- fixed tuples;
- immutable records;
- tagged unions; and
- `Actor.Ref<T>`/`Actor.Reply<T>` whose payloads are also accepted.

The first release rejects mutable arrays, lists, tables, classes, closures,
tasks, channels, resource/native handles, borrowed views, pins, compiler
handles, and untyped external data. An immutable collection can enter the set
only through a focused accepted decision.

Local send copies the complete message graph into the receiver's ownership
domain. Sharing immutable runtime bytes is permitted only as an unobservable
optimization. No mutable reference points from actor state back into the
sender or shared heap.

Actor entry captures use the same rule and are copied before start. The inbox,
actor-local references, resources opened inside the actor, and child tasks
cannot escape. Actor state remains ordinary Pop data/functions rather than an
OOP base class or property bag.

## Panic isolation and supervision

An actor panic cancels and joins its children, runs cleanup, closes its mailbox,
and emits a typed `Actor.Exit` to supervisors and monitors. It does not unwind
through another actor. Runtime corruption, process-wide resource exhaustion,
and OS termination remain wider failures.

Every non-root actor has an explicit supervisor. A child specification contains
the typed entry factory, mailbox capacity, restart condition, shutdown
deadline, and resource limits. Initial strategies are:

- `Actor.Strategy.OneForOne`;
- `Actor.Strategy.OneForAll`; and
- `Actor.Strategy.RestForOne`.

Children start in declaration order and stop in reverse order. Restart count
and time window are bounded. Exhausting the intensity terminates the supervisor
under its parent's policy.

A restart creates a new actor incarnation. Old references remain stale and do
not silently retarget. A typed directory or application owner explicitly
publishes the replacement. Mailboxes and state are volatile; restart does not
promise durable state, replay, deduplication, or exactly-once processing.

Cancellation remains cooperative during shutdown. A missed deadline reports
an unresponsive actor; only an explicit wider OS-process policy can force
termination and forfeit managed cleanup.

## Distribution architecture

Distribution is an optional official `Pop.Cluster` Package. It depends on
`Actor`, `Codec`, `Net`, `Crypto`, `Identity`, and `Task`. Local
`Actor.Ref<TMessage>` and remote `Cluster.Actor<TMessage>` are distinct.

This distinction is mandatory:

| Local | Remote |
| --- | --- |
| `Actor.send(local, message, cancel)` | `Cluster.send(remote, message, cancel)` |
| runtime message copy | bounded schema encode, transport, decode, actor copy |
| local mailbox failure | authentication, protocol, transport, partition, and mailbox outcomes |
| no network capability | explicit node/transport capability |
| local ordering contract | ordering only within one connected sender/receiver session |

`Cluster.publish` explicitly exposes a local actor. `Cluster.spawn` starts only
a predeclared public actor entry recorded in deployed artifact/application
metadata. It cannot ship a closure, source, MIR, bytecode, native code,
compiler handle, or runtime symbol name.

Only public actor-message-safe schemas cross nodes. A handshake checks exact
Package/Bubble/type identity, schema version and fingerprint, limits, and
runtime capabilities. Initial compatibility is exact and fail-closed. Runtime
strings cannot resolve program types, actor entries, functions, or fields.

Production transports require mutual authentication and encryption. Message
size/depth, mailbox, connection, in-flight byte, decode allocation, and timeout
limits are mandatory. Test/in-process transports are explicitly identified and
cannot be mistaken for production security evidence.

The first delivery contract makes partial failure visible:

- no automatic retry or transparent rerouting;
- a successful send means remote mailbox admission, not processing;
- connection loss can return `Cluster.Delivery.Unknown` when admission cannot
  be proved either way;
- cancellation stops the local wait but cannot retract accepted remote work;
- disconnected does not imply dead; and
- exactly-once/durable behavior requires explicit `Message`/`Store`
  transactions, identifiers, and deduplication.

Remote monitors distinguish confirmed exit, stale incarnation, authentication
or schema rejection, timeout, transport loss, and unknown partition outcomes.
Remote placement/restart is an explicit bounded cluster-supervision policy, not
location transparency or an ambient global registry.

## Static effects and metadata

The closed effect vocabulary distinguishes at least:

- `Allocates` for task frames, mailbox nodes, copies, and encoded buffers;
- `Suspends` for await and coordination points that suspend a task;
- `Blocks` for native/OS-worker blocking;
- managed mutation/synchronization and GC safe points;
- panic/unwind and cancellation; and
- ambient I/O plus native transitions for cluster transports.

There is no unknown effect. Public documentation states allocation, copying,
backpressure, suspension/cancellation, blocking, ordering, supervision,
delivery uncertainty, limits, and security boundaries.

`.poplib` public metadata retains async calling convention and task type, actor
entry/message identities, recursive copy/wire layouts, schema fingerprints,
closed effects, visibility, and runtime capabilities. It does not retain actor
state, private mailbox data, scheduler queues, credentials, or a runtime type
registry.

## HIR and MIR contract

Typed HIR retains:

- async callable/completion/task identity;
- `Await` with exact operand/result type;
- task-group ownership and start-once facts;
- synchronous and async cleanup scopes;
- actor entry/inbox/message identities;
- actor-message/capture proof; and
- origin spans for every boundary.

Canonical MIR owns the coroutine state machine. Each suspend terminator names
the polled task/operation, resume state, cancellation cleanup edge, unwind
action, safe point, and exact live-frame map. No backend rebuilds liveness,
ownership, or cleanup from source.

Task, group, channel, and local actor transitions are closed typed MIR/PLRI
operations. Actor enqueue/dequeue includes the exact message type, copy map,
mailbox outcome, and safe-point behavior. Cluster protocols are ordinary typed
library code over existing backend-neutral I/O/runtime operations; they do not
become MIR network opcodes.

MIR verification rejects:

- wrong task/message/result types;
- double task start or missing owner;
- invalid resume predecessor/state;
- stale frame, root, or actor copy maps;
- skipped/reordered cleanup;
- suspension without a safe point;
- inbox or actor-local reference escape;
- shared-to-actor-local pointers;
- dynamic tag/name dispatch; and
- scheduler, socket, LLVM, or native ABI objects in canonical MIR.

## Runtime, GC, and backends

PLRI supplies versioned task/frame allocation, poll, suspend, resume,
cancellation, group, channel, actor mailbox, monitor, and supervisor
transitions. Scheduler data structures and native symbols remain private.

Every suspended frame is a precisely scannable heap-owned root container.
Actor state uses a scheduler-local or isolated ownership region. Message copy
and mailbox publication preserve the invariant that a shared object never
points into local young/actor memory. Task/actor migration performs required
copy, promotion, publication, or delay before changing scheduler ownership.

The MIR interpreter supplies a deterministic scheduler and in-process cluster
transport. LLVM and a future VM preserve the same local semantics. Production
timing stays nondeterministic inside the ordering contract. The experimental C
backend rejects tasks, suspension, channels, actors, and cluster-dependent
programs before emission.

## Diagnostics and conformance

Structured diagnostics cover async/type misuse, illegal suspension,
unstructured ownership, unsafe actor messages/captures, inbox escape,
unbounded-mailbox defaults, stale incarnation, schema/visibility mismatch,
hidden remote authority, missing limits/security, and unsupported backend
capabilities. A quick fix cannot silently change public async type, isolation,
serialization, visibility, authority, or remote reachability.

Required proof includes positive, negative, regression, deterministic replay,
GC stress, cross-backend, artifact round-trip, security/fault-injection, and
performance tests from ADR 0068. Benchmarks cover cold creation, ready-task
throughput, one million suspended frames on a declared reference profile,
channel/actor contention, mailbox pressure, supervision storms, actor-message
copy size, cluster encoding, connection loss, and tail latency. Results always
record target, worker count, memory limits, runtime/collector stage, workload,
and percentile methodology.

## Explicit non-goals

- dynamic/untyped task results or mailboxes;
- detached work as the ordinary spawn form;
- preemptive cancellation that skips cleanup;
- one native thread or OS process per actor;
- selective receive by runtime tag/name search;
- ambient current task, actor, supervisor, node, or global registry;
- mutable alias sharing across actors;
- transparent local/remote actor references;
- code/closure shipping;
- automatic retry, exactly-once, durable mailbox, or partition masking; and
- scheduler, GC-region, transport, or LLVM details in source-visible identity.
