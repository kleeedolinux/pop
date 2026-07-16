# ADR 0085: Proof-Directed Static Reclamation

- Status: accepted
- Date: 2026-07-15
- Supersedes: none
- Extends: ADR 0008, ADR 0019, ADR 0022, ADR 0039, ADR 0052, ADR 0068,
  ADR 0077, ADR 0079, ADR 0080, and ADR 0082

## Context

Pop GC already authorizes stack allocation, scalar replacement, escape
analysis, scoped arenas, ownership transfer, and region lifetime inference.
The runtime implements typed scoped bump arenas, and LLVM has one backend-local
optimization for non-escaping scalar arrays. The architecture does not yet
define one portable proof, lifetime frontier, or every-exit reclamation contract
that the interpreter, LLVM, and a future VM can share.

Leaving the optimization backend-local has three problems:

- a backend may use weaker escape rules than another;
- normal return can be handled while unwind, cancellation, or suspension paths
  are missed; and
- compiler-known safe arrays/aggregates still enter GC in every backend that
  does not reproduce the special case.

Pop should gain the benefit of ownership/lifetime compilation without adopting
Rust-shaped source syntax or rejecting a safe program merely because its
allocation needs tracing. The design also needs an honest boundary: exact
static liveness is not complete runtime reachability for arbitrary mutable,
cyclic, shared, concurrent, or foreign-retained graphs.

## Decision

Pop adopts **proof-directed static reclamation** before precise tracing GC.

The source language remains unchanged. Storage placement is inferred per
allocation use, not fixed per source type. The same `Array<T>`, record, class,
or closure type may be scalar-replaced, activation-owned, region-owned, or
managed according to its proven aliases and lifetime.

The mandatory rule is:

```text
complete verified proof -> static reclamation is permitted
missing or incomplete proof -> managed allocation through Pop GC
invalid proof -> compiler/verifier incident
```

"Exact" means that no valid execution can use or borrow the value after the
inserted lifetime end. It does not mean that analysis must optimize every safe
allocation. Analysis budget exhaustion and absent interprocedural facts fall
back to managed allocation.

### Storage plans

Optimized canonical MIR uses the closed storage plans:

- `Elided` for values with no physical allocation;
- `StaticSlot{LifetimeId}` for one activation/frame-owned allocation;
- `ScopedRegion{RegionId, LifetimeId}` for allocations with one common proven
  frontier;
- `Managed{AllocationClass}` for runtime-reachability lifetimes; and
- `Immortal` only for verified process-lifetime constants/metadata.

External/native resources remain separately and explicitly closed. They are
not memory plans.

An activation-owned dynamic-size array may use checked native side storage; a
backend is not required to place it on the machine stack. Canonical MIR states
the lifetime plan, while backend-private layout selects registers, frame slots,
native side buffers, or VM storage.

### Proof and lifetime frontier

Every candidate allocation has a stable `AllocationSiteId`. Portable analysis
computes its SSA aliases, managed/interior edges, identity observations,
borrows, calls, ownership transitions, safe points, cleanup, unwind,
cancellation, and suspension paths.

The lifetime frontier is the complete set of control-flow edges after the last
possible use. It may be path-specific and non-lexical. The compiler inserts one
balanced end/close on every applicable edge; cleanup and coroutine destruction
are part of the graph, not backend conventions.

The first closed proof kinds are:

- `NonEscapingAllocation`; and
- `CommonLifetimeRegion`.

The MIR verifier reconstructs each proof. Backends consume the verified plan
and do not independently infer a weaker lifetime.

Static/region storage may contain managed references only through exact root
maps. Managed storage cannot point into static/region storage. Region interior
references cannot escape. Same-region cycles are permitted because the whole
region closes atomically.

### Calls and separate compilation

Effects do not fully describe retention. ADR 0097 refines the original flat
vocabulary into one structured summary carried by function types/HIR and
cross-Bubble reference metadata:

```text
parameterRetention[i] = DoesNotRetain
                      | MayRetain
                      | StoresInto(targetParameter)
                      | Captures
                      | Publishes

resultProvenance[j] = Independent
                    | ReturnsAlias(sourceParameter)
                    | MayAlias
```

Missing or incompatible metadata selects `MayRetain` and `MayAlias`. These are
conservative static facts, not unknown runtime effects. They force ordinary
owned allocations toward managed storage but reject a borrowed view. Interface
implementations cannot retain an argument where the interface contract promises
`DoesNotRetain` or change an exact result provenance. Summary/proof versions
participate in cache and artifact invalidation.

### Required tracing boundary

Pop GC remains mandatory when the object's death is known only by runtime
reachability and no exact enclosing region owns the graph. This includes
runtime-dependent shared aliases, mutable cycles, shared concurrent graphs,
escaping closure/capture state, scheduler- or mailbox-retained values,
long-lived actor state with shorter-lived internal garbage, strong managed
handles/callbacks, and any unproven case.

Actors are mixed: handler scalars and temporary arrays/graphs can be static or
region-owned; incarnation-wide state can use an isolated region; internal
garbage discarded before a long-lived actor terminates still requires local
tracing unless a smaller lifetime proof exists.

Pop does not introduce per-object reference counting as the automatic fallback.
Any future counting/tracing collector remains a runtime strategy requiring its
own ADR and must preserve this semantic contract.

### Source and resource behavior

No lifetime parameter, ownership modifier, `move`, `borrow`, `delete`, `free`,
stack/arena selector, implicit destructor, weak reference, or finalizer is
added. Failure to prove a static plan is not a source diagnostic by default.

`defer`, `async defer`, `Ffi.Buffer<T>`, pins, handles, callbacks, task groups,
and actor/isolated ownership keep their existing explicit contracts. Memory
liveness never substitutes for external-resource cleanup.

## Consequences

- Proven scalars, arrays, aggregates, and closure environments can avoid GC
  allocation, barriers, tracing, and relocation.
- Programs keep the existing small Luau-shaped source surface and GC fallback.
- Canonical MIR gains lifetime/region identities, plans, operations, call
  summaries, and verifier obligations.
- Static slots/regions containing managed references remain precise relocating
  root containers until their verified end.
- Separate-Bubble metadata compatibility now includes retention summaries.
- Optimization may become more conservative without changing accepted source
  programs; it may never become less sound.
- Compiler time, code size, materialization cost, stack/region peak memory, and
  GC work avoided require telemetry and paired benchmark gates.

## Alternatives considered

### Keep the LLVM-only scalar-array optimization

Rejected because backend-local inference does not prove common interpreter/VM
behavior, every-exit close, or a reusable memory contract.

### Require explicit Rust-like ownership and lifetimes

Rejected because Pop Lang's identity requires a lightweight Luau-shaped
surface. Static optimization can be inferred, and safe unproven programs can
use GC instead of becoming source errors.

### Make every value region-allocated

Rejected because one coarse region can retain short-lived data until a
long-lived owner exits. Shared aliases, runtime mutation, suspension, and
independent lifetimes still require managed reachability or smaller proven
regions.

### Use automatic reference counting after proof failure

Rejected because shared atomic updates, recursive release, and cyclic garbage
would add a second universal runtime tax and cycle contract. Precise tracing is
the accepted safe fallback.

### Free every object at compiler-estimated last use

Rejected because an estimate is not a proof. Hidden aliases, indirect-call
retention, cleanup, concurrency, and foreign boundaries can keep an object
live beyond one apparent SSA use.

### Treat actors as wholly static regions

Rejected because bulk reclamation only at actor termination retains garbage
created and discarded during a long-lived actor's lifetime. Actor-local static
subregions and tracing are complementary.

## Required conformance tests

- scalar, tuple, record, class-identity, fixed-array, dynamic scalar-array, and
  closure-environment elision/static-slot positives;
- escaping return, capture, global/managed store, publication, task/channel/
  actor send, handle/callback, FFI, and unsupported-suspension negatives;
- branch/loop/path-sensitive last use and normal, result-failure, return,
  loop-control, unwind, cancellation, and coroutine-destruction frontiers;
- same-region aliases/cycles, exact outward managed roots, forced relocation,
  bulk close, hard-limit failure, and cross-region/managed-to-region rejection;
- lifetime-summary inference, interface conformance, cross-Bubble round trip,
  generic capsule, stale cache, and absent-summary `MayRetain` fallback;
- verifier rejection of forged proofs, wrong allocation membership, use after
  end, missing/duplicate close, missed exit, stale root, identity-changing
  materialization, and wrong summary;
- construction/optimized MIR plus MIR-interpreter/LLVM/future-VM differential
  results, traps, allocation events, and forced-GC behavior;
- deterministic analysis-budget exhaustion that retains managed allocation;
- permanent regressions forbidding source lifetime syntax, manual free,
  implicit destructors/finalizers, universal reference counts, conservative
  roots, backend-specific MIR, and dynamic fallback; and
- benchmarks for compile time, code size, managed bytes/collections avoided,
  activation/region peak memory, throughput, and tail latency.

## Documents/components affected

Language model, compiler pipeline, type/effect and callable summaries, HIR,
MIR, pass manager and verifier, PLRI/runtime memory operations, GC roots and
scoped arenas, closure conversion, coroutine frames, reference metadata,
generic capsules, MIR interpreter, LLVM, future VM, diagnostics/tooling,
architecture conformance, implementation roadmap, benchmarks, and Pop GC.
