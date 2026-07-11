# ADR 0022: Explicit Effects, Failures, and GC Operations in Portable IR

- Status: accepted
- Date: 2026-07-10
- Supersedes: none

## Context

Pop Lang already distinguishes typed expected failures, runtime traps, and panic
unwinding, and already requires precise GC operations. The existing conceptual
MIR list did not define how verified calls and failure/control-flow edges carry
those facts portably.

## Decision

Every function type, HIR function, MIR function, and call site carries a
canonical effect summary. The initial summary records allocation, managed
mutation, possible trap, possible panic/unwind, suspension, unsafe memory, FFI,
ambient I/O, and permitted compiler-query capabilities. A caller may state a
strict superset of a callee's effects, never a smaller set. There is no
`Unknown` or dynamic effect.

The initial source surface has no effect punctuation. After typed bodies and
resolved call identities exist, the compiler computes the least fixed point of
local operation effects and direct-call edges for each recursive call-graph
component. A closure function type receives the resulting closed summary;
calls through function values use that summary rather than an all-effects or
unknown fallback. Interface summaries are the exact declared member summaries,
and an implementation may not widen them. Compile-time query capabilities are
present only on eligible compile-time functions and cannot escape into runtime
function types.

Expected recoverable failures remain ordinary typed `Result<T, E>` values.
Runtime traps use a closed backend-neutral `TrapKind`; checked operations name
their possible trap explicitly, and an unconditional trap is a terminator.
Traps do not become catchable exceptions.

Panic uses a typed runtime-private `PanicPayload` and unwinds. A call records an
explicit unwind action: propagate to the caller or branch to a verified cleanup
block. Cleanup blocks end in normal control flow or `resumeUnwind`; an
unconditional panic is a terminator. MIR contains no general source exception,
throw-by-name, or dynamically typed payload.

Managed allocation and collection are explicit PLRI operations. Canonical MIR
distinguishes object, closure-environment, array, and table allocation; logical
object maps identify managed fields; stack maps identify live managed values at
each `gcSafePoint`; root/handle/pin transitions and reference stores/barriers
are typed operations. Calls state whether they are GC safe points. Loop
backedges and long straight-line regions receive polls under a deterministic
bounded-work rule.

The bootstrap runtime is a precise stop-the-world collector with stable managed
handles and mark/sweep storage. It validates maps, roots, allocation, and safe-
point integration without pretending to implement the later moving nursery or
concurrent mature collector. The production collector stages remain those in
the GC architecture.

## Consequences

- Backends need no source reconstruction to implement traps, cleanup, or GC.
- Optimizers must preserve or soundly reduce effect summaries and root maps.
- PLRI contracts remain independent of compiler arenas and backend-specific
  pointer/layout types.
- The reference interpreter can record portable runtime events for differential
  testing.

## Alternatives considered

### Infer failure and GC behavior again in each backend

Rejected because it makes backend disagreement likely and violates canonical
MIR ownership.

### Model traps as ordinary `Result` values

Rejected because arithmetic/bounds traps are invariant failures, not expected
recoverable API results.

### Treat every word as a possible GC pointer

Rejected because conservative scanning contradicts the moving-nursery contract.

## Required conformance tests

- exact call-effect subset verification and no unknown-effect fallback;
- deterministic least-fixed-point inference for direct recursion, mutual
  recursion, closures, and interface implementations;
- checked-operation traps, unconditional traps, panic propagation, cleanup, and
  resumed unwind;
- object/array/environment allocation maps and precise stack maps;
- root dominance/balance, barrier placement, safe-point eligibility, and
  non-reference precision negatives;
- bootstrap reachability, cycles, reclamation, forced collection, deterministic
  out-of-memory behavior, and no finalizer/weak behavior;
- construction/optimized MIR and reference/bootstrap-runtime differential event
  traces.

## Documents/components affected

Type system, HIR, MIR, PLRI, native bootstrap runtime, interpreter, optimizer,
textual MIR tests, backend API, and conformance runner.
