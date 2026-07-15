# ADR 0080: Shared Immutability and Barrier Proofs

- Status: accepted
- Date: 2026-07-15
- Depends on: ADR 0001, ADR 0003, ADR 0008, ADR 0022, ADR 0039, and ADR 0077
- Supersedes: none

## Context

The collector already keeps ownership distinct from placement, generation, and
pinning, and it can publish a complete scheduler-local graph into shared
ownership. Shared ownership alone does not prove immutability: synchronized
mutable shared objects remain valid and require SATB barriers while shared
marking is active.

The GC architecture permits barrier elimination only from conservative compiler
proofs. The current MIR represents every managed class-field mutation with an
explicit write barrier, but it has no closed proof vocabulary with which an
optimizer can state and the verifier can check why the runtime barrier is
unnecessary. Removing the instruction as an ordinary optimization would make
the safety argument disappear from verified MIR.

## Decision

### Immutable shared graphs

Object mutability is typed metadata distinct from ownership, generation,
placement, and pin state. The initial states are `Mutable` and
`SharedImmutable`; these names are runtime/compiler vocabulary and do not choose
new source syntax.

An explicit freeze operation accepts one shared root and traverses its complete
managed-reference closure. It succeeds atomically only when every reached
object already has shared ownership. On success every reached object becomes
`SharedImmutable`, and the returned statistics describe the exact frozen
closure. No operation can restore mutable state.

Scalar, reference, array, table, bulk, and backend-adapter mutation of a
`SharedImmutable` owner fails before SATB logging, card marking, placement
change, or payload mutation. Freezing does not change ownership and does not
turn a shared object into a scheduler-local or isolated value.

### Verified MIR barrier proofs

Canonical MIR keeps each managed store explicit and may attach exactly one
closed `BarrierElisionProof` to its preceding write-barrier operation. An
attached proof means the semantic managed store remains, while the runtime
barrier call is omitted by every backend.

The first accepted proof is `UnpublishedOwner`. The MIR verifier accepts it only
when the owner is a dominating allocation in the same basic block and every
operation from allocation through the store preserves unpublished ownership.
Calls, returns, branches, strong-handle publication, capture stores, unknown
effects, and other escapes invalidate the proof. Precise safe-point root
publication remains allowed because it neither changes semantic ownership nor
exposes the reference outside collector relocation. Optimization may introduce the
proof only after ordinary unproved MIR has passed verification, and optimized
MIR must pass verification again.

The proof vocabulary is backend-neutral. LLVM, the MIR interpreter, and future
VMs omit the runtime barrier for a verified proof; they do not independently
infer weaker facts. An absent or invalid proof requires the ordinary barrier.
The managed-reference write effect remains visible even when its barrier is
elided.

Additional proof kinds, capability inference, isolated-region proofs, and
source syntax require a later accepted architecture change and corresponding
negative conformance coverage.

## Consequences

- Shared mutability is not conflated with shared ownership.
- Immutable shared objects cannot be mutated through a lower-level runtime path.
- Optimized MIR preserves a reviewable, deterministic reason for every omitted
  runtime barrier.
- Backends share one proof decision instead of implementing divergent barrier
  heuristics.
- This slice adds no dynamic capability lookup, runtime reflection, raw pointer,
  backend-specific MIR, or new source syntax.

## Alternatives considered

### Treat every shared object as immutable

Rejected because synchronized mutable state and concurrent data structures are
part of the accepted shared-heap model.

### Delete barriers during backend lowering

Rejected because backend-local inference would bypass MIR verification and
could produce backend disagreement.

### Trust an unverified optimization annotation

Rejected because a forged proof would permit the collector to miss an edge.
The verifier must reconstruct the conservative unpublished-owner fact.

### Expose a source-level `shared immutable` spelling now

Rejected because the source capability and borrowing syntax is not yet closed.
The runtime and MIR contract can be completed without silently choosing it.

## Required conformance tests

- freezing a complete shared graph marks every reached object immutable without
  changing ownership or placement;
- freezing a graph containing scheduler-local or isolated ownership fails
  without partial state change;
- every scalar/reference/bulk mutation path rejects an immutable owner before
  barrier or payload mutation;
- optimized verified MIR attaches `UnpublishedOwner` only to a same-block,
  non-escaping allocation and keeps the managed-write effect;
- the MIR verifier rejects a forged proof after a call, escape, wrong owner,
  wrong slot, or nonmatching store;
- the MIR interpreter and LLVM omit the runtime barrier for the same verified
  proof and preserve program results;
- unproved managed stores retain their explicit runtime barrier; and
- no source syntax, dynamic lookup, backend-specific HIR/MIR, or unrestricted
  reflection is introduced.

## Documents/components affected

GC architecture, compiler pipeline and MIR verification, portable collector
ownership/access/barrier paths, MIR interpreter, LLVM lowering, closed design
decisions, implementation roadmap, architecture conformance, and `ROADMAP.md`.
