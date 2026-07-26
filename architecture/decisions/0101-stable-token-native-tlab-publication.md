# ADR 0101: Stable-Token Native TLAB Publication

- Status: accepted
- Date: 2026-07-26
- Depends on: ADR 0008, ADR 0038, ADR 0039, ADR 0070, ADR 0072, ADR 0077,
  ADR 0080, ADR 0099, and ADR 0100
- Supersedes: none

## Context

The ABI 1 native facade correctly maps managed allocations into stable mature
space, but every allocation still acquires one process-global runtime mutex.
The page allocator has scheduler-indexed mature cursors only behind that mutex,
so it does not yet implement the accepted GC architecture's common allocation
path: a scheduler/thread-local pointer bump with no global lock, registry
lookup, virtual call, or atomic operation.

Moving the cursor into native thread-local state is not sufficient by itself.
The stable managed token, page placement, complete initialized payload, exact
object map, ownership, tracing metadata, memory accounting, and allocation-site
identity must become visible as one publication. Collection must not miss a
locally initialized object, another scheduler must not validate an unpublished
token, and abandoned reservations must not become phantom live objects.

## Decision

The ABI 1 stable-token composition may issue one typed native allocation lease
to an explicitly bound mutator and scheduler. A lease reserves:

- one monomorphic mature-page range for one interned allocation-site layout;
- a contiguous stable-token and object-identity range;
- exact payload starts and compact side-metadata entries;
- a bounded local cursor and limit; and
- deterministic committed-memory and allocation-debt capacity.

Reservation and refill are serialized slow paths. The common allocation path is
owned by the bound mutator and performs only:

```text
validate the exact cached site and initializer width
select the current reserved payload start and stable token
advance the plain local cursor
write every initializer into the private page range
append one complete publication record to the local buffer
return the stable token
```

A site becomes TLAB-active only after its first observed allocation. The first
allocation uses the typed serialized path; a second allocation at the same
immutable descriptor reserves the bounded lease. This prevents one-shot sites
from reserving and abandoning a full batch while leaving repeated sites on the
same common path.

The thread-owned allocation cursor, current target-span validation, and
buffered managed-array store capability may share one internal native state.
An ADR 0102 adjacent allocation/store pair therefore initializes the next
object and performs the checked scheduler-local store during one TLS access.
The capability remains typed, revision-checked, scheduler-bound, and quiesced
before every serialized boundary.

It performs no process-global lock, collector registry lookup, virtual call, or
atomic operation. The object remains `UnpublishedOwner` until its complete
record is flushed. A local direct store may consume an exact proof for the most
recently initialized token, but a general direct page span cannot validate
reserved or partially initialized entries.

Every operation that can expose the token outside its scheduler-local graph
flushes the caller's publication buffer first. This includes:

- TLAB refill or allocation-site switch;
- a checked slow-path access or barrier;
- root/pin retention and task-frame publication;
- local-to-shared or isolation ownership transitions;
- safe-point acknowledgement;
- managed-to-foreign transition;
- scheduler migration, managed detach, and thread shutdown; and
- collection or memory-pressure assist entry.

Flush occurs while holding the stable runtime's mutation authority. It
validates the exact binding and lease generation, installs complete object
records in token order or one compact immutable page-span directory record
that can deterministically materialize those exact records, advances each
page's published-token watermark only after installation, records
allocation-site and memory telemetry, and then clears the local publication
records. A compact record retains the exact token/identity range, page payload
starts, shared object map, type, allocation class, scheduler ownership, and
initialized count. Every slow access, root/ownership transition, tracing or
collection boundary materializes it before consuming per-object state. The
segmented object directory accepts disjoint reserved token ranges published out
of reservation order without reusing a token.

A mutator cannot acknowledge a collector epoch until its publications are
flushed and its current TLAB top is published. Other mutators and collector
workers therefore observe either no object or the complete typed object.
Unconsumed reservation capacity is page free space, not a live object. Detach,
site replacement, and thread shutdown cancel that capacity or return it through
the ordinary page-reuse policy. Failure before publication returns no token;
failure after local initialization is resolved by a successful flush or a
typed runtime failure before external visibility.

This is an internal implementation of the existing ABI 1.20 allocation
operation. It does not expose raw payload addresses through PLRI, change MIR,
enable native nursery movement, widen object ownership, or make the production
concurrent collector selectable.

## Consequences

- Repeated fixed-layout native allocation can use the accepted lock-free local
  pointer-bump path while preserving stable ABI 1 tokens.
- Allocation-site descriptors, page layouts, access, barriers, and tracing
  continue to share one typed layout identity.
- Publication becomes an explicit bounded mutator record rather than an
  incidental consequence of holding the process-global mutex.
- One flushed lease may remain a compact typed page-span directory record until
  a collector or slow operation requires per-object state; this does not defer
  payload initialization or token publication.
- Token ranges may contain permanently unused reservation holes; holes are
  never valid managed references and are never reused.
- Refill, flush, safe points, collection, ownership transitions, and memory
  pressure remain checked slow paths.
- ABI 2 writable-root work and production concurrent collection remain
  separate requirements.

## Alternatives considered

### Prepublish zeroed objects and root the unused batch

Rejected because it changes allocation/OOM timing, creates phantom live
objects, inflates roots, and exposes identities before complete source
initialization.

### Update one global published-token watermark atomically per allocation

Rejected because the accepted common local allocation path performs no atomic
operation and a watermark alone cannot publish exact object metadata.

### Let the collector inspect another thread's local buffer directly

Rejected because it introduces a data race and bypasses ADR 0077's explicit
mutator acknowledgement and publication boundary.

### Encode a raw page address in the managed token

Rejected because managed references remain opaque stable identities and raw
managed addresses cannot cross PLRI or survive the future ABI 2 transition.

## Required conformance tests

- two or more allocations at one verified fixed-layout site reuse one
  scheduler/thread-local lease without another process-global runtime lock;
- each returned token is nonzero, distinct, stable, and exposes its complete
  initializer values only after local initialization;
- another scheduler rejects an unflushed token and accepts only a correctly
  published ownership transition;
- slow access, roots, pins, foreign transition, safe point, migration, detach,
  and shutdown flush every consumed publication exactly once;
- a major-collection handshake cannot acknowledge a managed mutator with
  unpublished allocation records or an unpublished TLAB top;
- out-of-order flush of two disjoint token reservations preserves both object
  directory entries and never validates unused holes;
- compact published spans and eagerly materialized object records have the same
  access, ownership, tracing, relocation, and memory-accounting behavior;
- initializer failure, memory-limit failure, stale binding, and abandoned
  reservation expose no partial object and do not reuse a token;
- tracing uses the exact allocation-site object map after flush, including
  scalar-equals-token negative coverage;
- MIR interpreter/native behavior and LLVM `-O3` retained-object checksums stay
  equal; and
- ADR 0099's paired 50-sample gate passes before the native fast path closes.

## Documents/components affected

GC and runtime architecture, native stable composition, scheduler/mutator
binding, allocation-site descriptors, segmented token storage, memory
accounting, native allocation facade, collector/native conformance tests,
benchmark evidence, and implementation roadmaps.
