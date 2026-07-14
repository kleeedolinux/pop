# ADR 0057: Typed Async Tasks, Isolated Actors, and Distribution

- Status: accepted
- Date: 2026-07-14
- Supersedes: the async, awaiting, cancellation, suspended-cleanup, and
  structured-concurrency deferrals in ADRs 0030, 0032, 0052, and 0053

## Context

Pop Lang reserves `Task<T>`, `CancelToken`, typed channels, coroutine runtime
transitions, and the closed `Suspends` effect, but it does not yet define the
complete source and runtime contract that connects them. Architecture examples
use proposed `async function` and `await` spellings, while ADR 0052 deliberately
limits ordinary `defer` to non-suspending cleanup.

The language also needs a concurrency model suitable for large servers and
distributed applications. Lightweight tasks alone do not isolate mutable
state or failures. Erlang-style processes and supervision supply those
properties, but copying Erlang's dynamically typed mailboxes, symbolic process
registration, or location-transparent distribution would violate Pop Lang's
static type, capability, and cost contracts. Go-style independent goroutines
are cheap, but unconstrained detached work would conflict with explicit
ownership and deterministic cleanup.

Leaving these boundaries implicit would let a backend choose task start
behavior, cancellation timing, frame layout, mailbox ordering, failure
propagation, or remote-delivery semantics. It would also risk copying Lua's
untyped resume/yield value channel or hiding network authority behind a local
call.

## Decision

### Distinct concurrency concepts

Pop Lang uses four distinct concepts:

| Concept | Purpose | Sharing and lifetime |
| --- | --- | --- |
| `Task<T>` | one typed asynchronous computation | lexically owned; may share ordinary state only under checked race-safety rules |
| `Channel<T>` | typed coordination and backpressure between tasks | endpoints are explicit; values retain their ordinary static ownership rules |
| `Actor.Ref<TMessage>` | one local isolated mailbox and failure boundary | messages are copied into private actor ownership; mutable actor state is never shared |
| `Cluster.Actor<TMessage>` | one explicitly remote actor endpoint | messages are schema-encoded and cross an authenticated transport with typed delivery outcomes |

An actor is not a task, OS process, Module, Bubble, Package, namespace, table,
or class. It may be implemented by a task tree and an isolated GC region, but
that implementation sharing is not observable. A cluster node is a runtime
deployment concept and never changes the Item → Module → Bubble → Package →
Workspace hierarchy.

### Async functions and task values

An async declaration uses one Luau-shaped modifier:

```luau
async function loadPage(uri: Uri, cancel: CancelToken): Result<Page, Http.Error>
    return await Http.get(uri, cancel)
end
```

The declared result is the value produced when the function completes. Calling
`loadPage` has static type `Task<Result<Page, Http.Error>>`. The async body is a
typed stackless coroutine. A call creates a cold task and evaluates its
arguments exactly once, but does not execute the body. Direct `await` owns and
starts that task; `Task.start(group, task)` transfers a cold task into an
explicit `Task.Group` and makes it eligible to run concurrently.

There is no detached task by default and no ambient source-visible current-task
object. A host may expose an explicit long-lived application supervisor, but
submitting work to it is an authority-bearing operation with a documented
shutdown owner; it is not a loophole around structured lifetime.

`Task<T>` is a reserved nominal `Pop.Standard` type. A task has exactly one
internal transition sequence from `Created` through running and suspension to
one terminal completion, cancellation, or panic state. Starting a task twice is
rejected by the typed ownership API. Completion is retained, so subsequent
typed awaits observe the same completed value without rerunning the body.
These states are runtime metadata, not a public reflective enum.

Async function types use `async function(parameters): ResultType`. Async and
synchronous function types are distinct; neither converts implicitly to the
other. Async closures use the same `async function(...) ... end` modifier and
capture only statically typed values under the ordinary capture rules.

### Scheduler and lightweight execution

Tasks are user-space execution units, not one operating-system thread each.
The native runtime uses an M:N scheduler over a bounded worker set. Ready work
may be stolen between workers, while suspended frames consume storage
proportional to their compiler-proven live state rather than a reserved native
thread stack. The architectural scale target is thousands or millions of
suspended tasks, subject to explicit process memory limits and measured
performance gates rather than an unconditional source-level count guarantee.

Scheduling is cooperative at semantic safe points but not dependent on
voluntary application calls alone. Await, channel/actor operations, allocation
slow paths, calls that may block, loop backedges after bounded work, and
compiler-inserted polls are scheduling and cancellation opportunities. A task
that performs bounded computation between polls cannot indefinitely starve
ready tasks. The language promises no wall-clock execution order between
independent ready tasks.

Blocking native or system work declares `Blocks` separately from `Suspends`.
An async API must use a nonblocking adapter or an explicit bounded blocking
pool; it cannot silently pin a scheduler worker. Unsafe FFI declares its
blocking, callback, root, unwind, and cancellation behavior. Scheduler policy,
worker count, and work-stealing queues are runtime details behind PLRI.

### Awaiting

Prefix `await expression` evaluates its operand exactly once. The operand must
have exact type `Task<T>`, the enclosing callable must be async, and the
expression has type `T`. Awaiting a completed task does not suspend. Otherwise
the current coroutine publishes its precise live roots, suspends, and resumes
at one compiler-defined continuation when the task makes progress or reaches a
terminal state.

`await` is a cancellation point and invalidates flow narrowing across the
suspension unless the effect system proves that the narrowed place cannot be
mutated or aliased. It cannot occur in constants, compile-time evaluation,
attributes, synchronous functions, ordinary `defer`, or unsafe/FFI regions that
hold an untracked managed pointer or active pin across suspension.

An awaited task panic crosses the await boundary as ordinary panic unwinding.
An awaited task cancellation exits the current async callable through its
`Cancellation` cleanup chain and completes its task as cancelled. Cancellation
is not `nil`, a dynamic exception, or an implicit `Result` error.

### Explicit cancellation and structured ownership

`CancelToken` is an immutable, cheaply copied typed value. Cancellation is
requested through its owning `Task.CancelSource` or through a `Task.Group`;
APIs that accept cancellation receive a token explicitly. Tokens do not grant
ambient I/O or scheduler authority and cannot be recovered by runtime name
lookup.

Cancellation is cooperative. It is observed at `await`, explicit task yield,
channel and actor suspension, scheduler polls, and accepted cancellation-aware
library operations. A request remains pending until one of those points.
Cancellation never destroys a frame asynchronously or skips cleanup.

`Task.group(cancel, body)` creates a lexical owner for every task started
through the supplied `Task.Group`:

```luau
async function loadBoth(cancel: CancelToken): Result<(Page, Page), Http.Error>
    return await Task.group(cancel,
        async function(group: Task.Group): Result<(Page, Page), Http.Error>
            local first = Task.start(group, Http.get(firstUri, cancel))
            local second = Task.start(group, Http.get(secondUri, cancel))
            return Result.Ok(await first, await second)
        end)
end
```

On body completion, result failure, panic, or cancellation, the group requests
cancellation for unfinished children and awaits every child cleanup before the
group itself completes. A child panic cancels siblings and propagates only
after they join. Children cannot outlive the group, and ownership is represented
by typed task/group identities rather than a global scheduler registry.

### Suspension-capable cleanup

Ordinary `defer ... end` retains ADR 0052 semantics and cannot suspend. An async
callable may register suspension-capable cleanup with `async defer ... end`:

```luau
local stream = try await File.open(uri, cancel)
async defer
    await File.close(stream)
end
```

Registration, lexical reachability, exactly-once execution, and last-in,
first-out ordering match ordinary `defer`. Async cleanup runs on fallthrough,
return, typed-result propagation, loop control, panic unwind, and cancellation.
Once entered because of cancellation, cleanup is cancellation-masked: the
original request remains pending but cannot skip a later registered cleanup.
Awaited cleanup can still panic, and the ADR 0052 double-panic terminal rule
applies during panic cleanup.

An async cleanup body cannot contain `return`, `break`, `continue`, prefix
`try`, or another cleanup declaration. Every task result awaited by cleanup
must be consumed according to its static type. Managed values captured by the
cleanup remain precise coroutine-frame roots until it completes.

### Typed channels and selection

`Channel<T>` has exact sender and receiver endpoint types. Sends and receives
are task operations and cancellation points. Bounded channels are the default
for streams and pipelines; capacity and the behavior of closure are explicit.
An unbounded channel is an advanced operation with documented allocation and
memory-limit behavior, never an invisible default.

`Task.select` accepts a closed statically typed set of receive, send, task, or
timer cases. Each case retains its exact result type and stable source order.
If exactly one case is ready it is selected. If several are ready, selection is
scheduler-chosen unless an explicit priority policy is supplied; no hash or
runtime name decides it. The deterministic test scheduler records and replays
that choice. Selection never yields an untyped `(tag, value)` pair.

Channels coordinate tasks within a sharing domain. They do not create actor
isolation, supervision, durable messaging, or a transparent network transport.

### Actor isolation and typed mailboxes

`Actor` is a standard/platform public root. An actor owns:

- one incarnation-scoped `Actor.Ref<TMessage>`;
- one bounded FIFO mailbox of exactly `TMessage`;
- one private mutable state and resource domain;
- one structured child-task tree; and
- one panic/cancellation/normal-exit boundary.

The first release adds no `actor` declaration syntax. An actor entry is an
ordinary async closure passed to `Actor.start`. It receives a non-escapable
`Actor.Inbox<TMessage>` and explicit `CancelToken`, so state remains ordinary
locals, records, unions, collections, and resource handles:

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

local counter = try await Actor.start(supervisor, {
    capacity = 256,
}, runCounter)
```

The actor entry's captured inputs are copied into the actor before it starts.
The checker accepts only recursively actor-message-safe captures. The inbox,
actor-local references, borrowed views, pins, resource handles, tasks, channels,
closures, and mutable class/collection identities cannot escape the actor or
be captured from its caller.

An actor message type is proven recursively by the compiler; this is a closed
type property, not a source-visible marker interface or runtime reflection
test. The first accepted set contains primitives, strings, enums, fixed tuples,
immutable records, tagged unions, and actor reference/reply capabilities whose
components are also accepted. Mutable arrays, lists, tables, classes, closures,
tasks, channels, native handles, borrowed/pinned values, and untyped external
data are rejected. A later immutable collection can participate only through a
focused type/library decision.

Sending locally copies the complete message value into receiver ownership.
Immutable runtime storage such as string bytes may be shared as an
unobservable optimization. The receiver never observes a mutable alias into
the sender. `Actor.send` returns `Task<Result<(), Actor.SendError>>`; a full
mailbox applies backpressure, and completion means accepted by that mailbox,
not processed by the actor. `Actor.trySend` is non-suspending and reports full,
closed, or stale-incarnation outcomes explicitly. `Actor.Reply<T>` is a typed
single-use reply capability, not a general mailbox or selective-receive tag.

The mailbox is FIFO for messages accepted from one sender to one actor
incarnation. Interleaving between different senders is unspecified. The first
release has no selective receive: the actor consumes the next message and
exhaustively matches its union. This keeps queue cost bounded and avoids
runtime tag/name searches.

Only the actor entry processes mailbox messages, one at a time. It may suspend
while handling one message; later messages remain queued and cannot reenter the
handler. Child tasks belong to the actor's structured group and cannot mutate
actor-local state after the handler or actor lifetime that authorized them.

### Failure isolation and supervision

An actor is a panic boundary. A panic cancels and joins its child tasks, runs
registered cleanup, closes the incarnation mailbox, and publishes one typed
`Actor.Exit` event to its supervisor/monitors. It does not unwind through an
unrelated actor. Runtime corruption, process-wide resource exhaustion, and
host termination remain wider failure domains and are never mislabeled as an
isolated actor panic.

Every non-root actor has an explicit `Actor.Supervisor`. A child specification
contains a typed entry factory, mailbox capacity, restart condition, shutdown
deadline, and resource limits. The initial restart strategies are
`OneForOne`, `OneForAll`, and `RestForOne`. Restart intensity is bounded by a
maximum count within a time window; exceeding it terminates the supervisor
according to its own parent policy. Children start in declaration order and
stop in reverse order.

Restart creates a new actor incarnation and mailbox. An old `Actor.Ref` remains
stale and never silently retargets. Typed directories or application-owned
references may publish the replacement explicitly. In-memory mailboxes are
volatile: supervision does not imply durable queues, exactly-once processing,
or state recovery. Those require explicit `Message`/`Store` contracts.

Supervision never forcibly tears down a managed frame and skips cleanup. If an
actor fails to reach cancellation polls before its shutdown deadline, the
supervisor reports it as unresponsive; only a wider host/OS-process policy may
terminate the process.

### Explicit distribution

Distributed actors live in the optional `Pop.Cluster` Package under the
`Cluster` public root. Local `Actor.Ref<TMessage>` and remote
`Cluster.Actor<TMessage>` are distinct types. A remote send is spelled through
`Cluster.send`, requires an explicit node/transport capability, records
`AmbientIo | Suspends`, and returns a typed delivery result. No overload makes a
network hop look like `Actor.send`.

Only public actor-message-safe types with emitted schema metadata may cross a
node boundary. The handshake uses exact Package/Bubble identity, public type
identity, schema version/fingerprint, protocol limits, and runtime capability;
it never resolves a type, actor, or function from an arbitrary runtime string.
The first release requires exact compatible schemas and fails closed. Messages
are encoded with bounded depth/bytes and decoded into a fresh receiver-owned
value before mailbox admission.

`Cluster.publish` explicitly exposes a local actor through an authenticated
node and returns `Cluster.Actor<TMessage>`. `Cluster.spawn` can start only a
predeclared public actor entry recorded in the deployed `.poplib`/application
manifest with an exact typed argument schema. It cannot transmit closures,
source, MIR, bytecode, native code, compiler handles, or arbitrary symbol
names.

The baseline transport contract is:

- mutual authentication and encryption are required outside an explicitly
  declared test/in-process transport;
- mailbox, message, connection, in-flight-byte, and decode limits are
  mandatory;
- one connected sender/receiver session preserves accepted message order for
  one target incarnation;
- connection loss may produce `Cluster.Delivery.Unknown` when the sender
  cannot prove whether the receiver accepted a message;
- no automatic retry, deduplication, exactly-once claim, or durable storage is
  implied; and
- cancellation stops the local wait but cannot retract a message already
  accepted remotely.

Remote monitoring distinguishes confirmed actor exit, stale incarnation,
transport loss, authentication failure, protocol mismatch, timeout, and an
unknown partition outcome. A disconnected node is not proof that its actor
terminated. Remote restart/placement is an explicit cluster-supervision policy
with bounded retries and stable deployment identities, not location
transparency or an ambient global registry.

### First-release coroutine boundary

Async state machines are the first-release coroutine contract. Pop Lang does
not add Lua's untyped `coroutine.resume`/`yield` multiple-value channel, runtime
status strings, or dynamically typed generator protocol. A future public typed
generator or async iterator may build on the same suspension machinery only
through a separate ADR with exact yield/result types. Actor mailboxes do not
serve as a dynamic coroutine resume channel.

### HIR, MIR, runtime, and backends

HIR records async callable identity, declared completion type, exact task type,
typed `Await` expressions, synchronous versus async cleanup scopes, task-group
ownership, actor entry/message types, and compiler-proven actor-message safety.
It does not contain scheduler queues, sockets, native ABI objects, or remote
runtime names.

Canonical MIR represents an async callable as a verified coroutine frame and
explicit state machine. The frame records exact stored local/capture types,
precise managed-root slots, cleanup state, and resume-state identities. Await
lowers to a typed task poll plus a `Suspend` terminator naming the awaited task,
resume block, cancellation cleanup edge, unwind action, and live-frame map.
Resume is an explicit entry with one predecessor state. No backend reconstructs
spill liveness or cleanup order from source.

Task/group/channel/actor operations are closed backend-neutral MIR/PLRI
operations with exact generic types and stable operation IDs. Actor start
records the entry function and message layout; enqueue/dequeue records exact
message type, copy map, mailbox result, cancellation edge, and safe point.
Cluster encoding, transport, discovery, and policy are ordinary typed
`Pop.Cluster` library code over `Codec`, `Net`, `Crypto`, `Identity`, `Task`,
and `Actor`; they do not become compiler opcodes.

Every suspension and resumption is a GC safe point. A suspended frame is a
precisely scannable heap-owned root container and does not need to resume for
collection. An actor's mutable graph remains in one scheduler-local/isolated
ownership domain; mailbox publication cannot create a shared-to-local pointer.
A task or actor may migrate only under the accepted GC publication, copy, or
promotion rules.

The MIR interpreter, LLVM, and a future VM preserve identical start, polling,
completion, cancellation, panic, cleanup, mailbox, supervision, and local
actor semantics. Tests use an injected deterministic scheduler and in-process
cluster transport to record/replay allowed choices. Production scheduling and
network timing remain nondeterministic within the stated ordering contract.
The experimental C backend rejects async frames, tasks, channels, actors, and
cluster operations before emission.

### Effects, artifacts, diagnostics, security, and costs

Calling an async function allocates task/frame state and is recorded as
`Allocates`; executing or awaiting it is `Suspends` and a GC safe point.
Actor start/send/receive additionally records allocation, message copying,
synchronization, and possible suspension. Cluster operations record explicit
ambient I/O, encoding/allocation, authentication, and suspension effects. An
async spelling never hides blocking I/O.

Async function references and `.poplib` metadata retain the async calling
convention, completion/task types, and closed effects. Public actor entries
retain exact message identities, recursive copy/wire layouts, schema
fingerprints, visibility, effects, and required runtime capabilities. Only
public declarations enter consumer metadata; actor state and private mailbox
contents never become reflection data.

Structured diagnostics reject awaiting a non-task, awaiting outside async code,
missing task ownership, illegal suspension in cleanup/FFI/compile time,
invalid async conversion, unsafe actor messages/captures, inbox escape,
unbounded mailbox defaults, stale actor references where statically known,
schema mismatch, hidden remote authority, and unsupported backends. Fixes
cannot make a callable async, actor-safe, public, serializable, or remotely
reachable when that changes its public type, ownership, or security contract
without review.

Public concurrency APIs document task/frame/mailbox allocation, copy versus
share behavior, suspension/cancellation points, scheduler/actor ownership,
cleanup, buffering/backpressure, ordering, restart limits, blocking/native
transitions, message and connection limits, authentication, and delivery
uncertainty. Performance claims require the versioned scheduler/actor/cluster
benchmark suite.

### External influence boundary

The design deliberately adapts, rather than copies, these official models:

- Go's specification defines goroutines as independent concurrent execution
  and typed channels as communication/synchronization; Pop adopts lightweight
  runtime scheduling and typed channels but rejects discarded results and
  detached work as the default:
  <https://go.dev/ref/spec#Go_statements> and
  <https://go.dev/ref/spec#Channel_types>.
- Crystal documents lightweight cooperatively scheduled fibers and channels;
  Pop adopts the direct async feel and lightweight execution target while
  retaining backend-neutral tasks, parallel scheduling, structured ownership,
  and explicit effects:
  <https://crystal-lang.org/reference/1.20/guides/concurrency.html>.
- Erlang documents isolated processes, asynchronous signal/message queues,
  per-sender ordering, links/monitors, and supervision trees; Pop adopts
  isolation, failure boundaries, and supervision while replacing arbitrary
  terms, selective dynamic receive, and symbolic registration with exact typed
  mailboxes:
  <https://www.erlang.org/doc/system/ref_man_processes.html> and
  <https://www.erlang.org/doc/system/sup_princ.html>.
- Elixir's `Supervisor` and `Task` APIs demonstrate hierarchical restart and
  linked async work; Pop keeps those lifecycle lessons while using native
  typed functions/data rather than behaviors, process dictionaries, or
  dynamically shaped messages:
  <https://hexdocs.pm/elixir/Supervisor.html> and
  <https://hexdocs.pm/elixir/Task.html>.

Erlang distribution is specifically not a security or location-transparency
template. Pop requires explicit remote types, capability-bearing secure
transports, schema handshakes, bounded decoding, and typed uncertainty rather
than a shared cookie or transparent PID operation.

## Consequences

- Async calls have one exact static task type and no dynamic resume protocol.
- M:N scheduling and compact suspended frames provide a goroutine/fiber-scale
  target without making tasks detached or one-thread-per-call.
- Cold tasks and lexical groups make ownership and cleanup reviewable.
- Cancellation, expected errors, panic, actor exit, and network uncertainty
  remain distinct.
- Actor-local mutation and copied typed messages provide Erlang-like isolation
  without a dynamic mailbox or shared mutable aliases.
- Supervision supplies bounded restart policy without implying durable state or
  exactly-once processing.
- Separate local and remote actor types keep network cost, authority, failure,
  versioning, and partial failure visible.
- Suspended frames and actor ownership domains preserve precise GC and backend
  equivalence contracts.

## Alternatives considered

### Use Lua-style coroutine resume and yield

Rejected because untyped multiple values, runtime status strings, and
caller-driven dynamic protocols violate strong static typing.

### Add a `go`/`spawn` statement that detaches any call

Rejected because discarded results, ambient parentage, and work that outlives
lexical resources conflict with structured cleanup and explicit ownership.

### Make every async call hot

Rejected because mere value construction would schedule hidden work and make
abandoned tasks observable. Explicit `Task.start` states concurrency at the
ownership boundary.

### Represent cancellation as a universal result error

Rejected because cancellation is a control exit shared by arbitrary result
types; implicit error widening would violate exact typed propagation.

### Permit await in ordinary defer

Rejected because synchronous cleanup call sites and effects would no longer
state whether leaving a scope can suspend.

### Use channels as the only isolation primitive

Rejected because channels coordinate communication but do not by themselves
prevent shared mutable aliases, contain panics, or define supervision.

### Copy Erlang processes and transparent distribution directly

Rejected because arbitrary term mailboxes, runtime atom/name resolution,
selective dynamic receive, transparent remote PIDs, and cookie-style trust
conflict with exact static types, bounded cost, and explicit security.

### Give each actor an operating-system process or thread

Rejected because it prevents the required lightweight scale and duplicates
`Process`/platform-thread concepts. Applications may deliberately use OS
processes as a wider fault/security boundary through the existing typed APIs.

### Promise exactly-once remote delivery

Rejected because partitions and acknowledgement loss make that promise false
without application-specific durable transactions and deduplication.

## Required conformance tests

- async declaration/function-type/closure parsing and exact `Task<T>` call
  typing, including synchronous/async conversion rejection;
- cold start, argument-once evaluation, explicit group start, start-once,
  repeated completed await, nested await, and exact result typing;
- M:N scheduling without one thread per task, bounded-work polls, ready-task
  progress, blocking-pool isolation, deterministic choice replay, and high
  suspended-task-count memory/latency benchmarks;
- await-outside-async, non-task await, compile-time/ordinary-cleanup/pinned-FFI
  suspension rejection, narrowing invalidation, and no detached default;
- explicit token propagation, cooperative cancellation points, lexical group
  ownership, child joining, sibling cancellation, and cancellation/panic/result
  separation;
- ordinary and async cleanup registration, LIFO order, every exit reason,
  cancellation masking, panic/double-panic behavior, and captured-root liveness;
- typed bounded/unbounded channels, endpoint direction, close/backpressure,
  cancellation, exact typed select cases, and record/replay of simultaneous
  readiness;
- actor-message recursive acceptance and rejection, copied capture/message
  isolation, no mutable alias escape, non-escapable inbox, FIFO per sender,
  cross-sender interleaving, bounded mailbox pressure, typed replies, stale
  incarnation rejection, and no selective runtime tag lookup;
- actor panic containment, child cleanup, monitor exits, supervisor start/stop
  order, each restart strategy, restart-intensity exhaustion, replacement
  incarnation publication, unresponsive shutdown, and volatile mailbox/state;
- cluster exact-schema handshake, public visibility, bounded codec failures,
  authenticated transport, explicit capability, per-session ordering, stale
  incarnation, cancellation, timeout, partition/unknown delivery, no automatic
  retry/exactly-once claim, and no code/closure shipping;
- HIR/MIR verifier negatives for wrong task/message types, invalid resume states,
  stale frame/copy maps, skipped cleanup, non-safe-point suspension,
  shared-to-actor-local pointers, and backend-specific state;
- interpreter, optimized MIR, and LLVM differential execution, precise GC
  stress over suspended frames and actor regions, deterministic scheduler and
  in-process cluster traces, plus explicit C rejection; and
- source-free `.poplib` async/actor/cluster reference and implementation round
  trips with exact effect, ABI, task, schema, visibility, capability, and
  dependency metadata.

## Documents/components affected

Vision, language model, syntax and nomenclature, type system, resolver and
signatures, capture/escape/flow/race analysis, HIR, MIR, optimizer/verifiers,
interpreter, LLVM, experimental C capability validation, PLRI/native ABI,
runtime scheduler, actor runtime, GC, `Pop.Internal`, `Pop.Standard`, optional
`Pop.Cluster`, artifacts, diagnostics, documentation, security, standard
library catalogs/examples/implementation plan, conformance policy, closed
decisions, and roadmap.
