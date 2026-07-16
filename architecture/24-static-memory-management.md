# Static Memory Management

## Status and authority

This document integrates
[ADR 0085](./decisions/0085-proof-directed-static-reclamation.md). It defines
the compiler/runtime contract for keeping proven values out of Pop GC while
preserving the same Pop Lang source semantics, object identity, safety, and
backend behavior.

The design extends, rather than replaces, the
[garbage collector architecture](./15-garbage-collector-architecture.md). Pop
GC remains the correctness-preserving fallback whenever the compiler cannot
prove a stronger storage lifetime.

## Objective

Pop Lang should not trace, relocate, barrier, or retain an allocation when the
compiler can prove exactly when its observable lifetime ends.

The common source model remains simple:

```luau
local values = Array.create<<Int>>(count, 0)
fillValues(values)
return sumValues(values)
```

The source does not select a stack, arena, heap, reference count, or explicit
`free`. The compiler may prove that `values` never escapes and select static
storage. If another use returns the same array, captures it, stores it into a
retained object, sends it, or keeps it across an unsupported suspension, the
compiler selects managed storage instead. The type remains `Array<Int>` in
both cases.

The goals are:

- keep scalars and scalar-replaced aggregates in SSA values, registers, or
  fixed activation storage;
- place proven non-escaping arrays, records, closure environments, and class
  instances in activation-owned storage;
- group allocations with one proven lifetime into compiler-inferred scoped
  regions and reclaim them in bulk;
- insert exact lifetime-end operations on every applicable control-flow edge;
- retain precise GC roots for managed references held inside static storage;
- preserve ordinary GC ergonomics when proof is incomplete;
- require no Rust-shaped ownership, borrowing, or lifetime syntax; and
- make the proof and storage decision backend-neutral and independently
  verifiable.

## Research basis

The selected design was reviewed against primary language documentation and
memory-management research.

- Tofte and Talpin's
  [region-based memory management](https://doi.org/10.1006/inco.1996.2613)
  infers region allocation and deallocation with type-and-effect analysis. It
  demonstrates that region placement can be compiler-generated rather than a
  mandatory source concern.
- Cyclone's
  [region architecture](https://www.cs.cornell.edu/Projects/cyclone/papers/cyclone-regions.pdf)
  integrates stack allocation, regions, and a garbage-collected heap safely.
  Pop adopts the layered memory lesson but rejects Cyclone's source-visible
  region annotations as a normal Pop programming requirement.
- Rust's
  [non-lexical lifetime model](https://doc.rust-lang.org/stable/edition-guide/rust-2018/ownership-and-lifetimes/non-lexical-lifetimes.html)
  shows why a borrow may end at its final use rather than at the textual end of
  a block, while Rust's
  [drop scopes](https://doc.rust-lang.org/reference/destructors.html#drop-scopes)
  deliberately define observable destructor timing separately. Pop adopts
  non-lexical proof precision but has no implicit user destructor whose timing
  would expose storage reclamation.
- OpenJDK's
  [escape-analysis description](https://cr.openjdk.org/~cslucas/escape-analysis/EscapeAnalysis.html)
  connects non-escape proofs to scalar replacement and documents why
  conservative control-flow and interprocedural boundaries leave some
  allocations on the managed heap.
- [GoFree](https://homes.cs.washington.edu/~mernst/pubs/explicit-free-cgo2025-abstract.html)
  demonstrates compiler-inserted freeing in a concurrent garbage-collected
  language without changing its programming model. Its evaluation is evidence
  for the direction, not a performance promise for Pop Lang.
- [Perceus](https://www.microsoft.com/en-us/research/publication/perceus-garbage-free-reference-counting-with-reuse/)
  demonstrates precise compiler-inserted reference-count operations and reuse
  for a strongly typed functional core. Its own concurrency and cycle limits
  are reasons not to make reference counting Pop's universal fallback.
- [LXR](https://arxiv.org/abs/2210.17175) demonstrates a runtime hybrid in
  which prompt local reclamation still needs tracing to identify cyclic
  garbage. Pop retains tracing for the corresponding reachability cases rather
  than exposing weak/unowned cycle-breaking obligations to ordinary source.
- [Spegion](https://arxiv.org/abs/2506.02182) explores implicit non-lexical
  regions without a substructural source type system. Pop shares the goal of
  concise implicit regions, but requires its proof to fit the existing typed
  HIR, canonical MIR, cleanup, coroutine, and GC contracts.

These systems do not authorize foreign syntax or semantics. They establish a
design space from which Pop selects a native, Luau-shaped, source-transparent
contract.

## Exactness means soundness, not universal classification

Pop uses **exact** in a deliberately strict sense:

> Whenever the compiler selects static reclamation, every possible valid
> execution has ended all uses and borrows before the storage is reclaimed.

This is a soundness guarantee. It is not a promise that static analysis will
classify every allocation optimally.

For a fixed verified MIR graph, the compiler can calculate exact SSA liveness,
control-flow-specific final uses, and the exits crossed by an allocation. It
cannot generally replace runtime reachability for a graph whose retaining
aliases depend on input, mutation, scheduling, callbacks, foreign code, or
arbitrary cycles. At any missing fact, analysis budget exhaustion, unsupported
construct, or unverifiable transformation, the allocation remains managed.

The fallback rule is mandatory:

```text
complete proof -> static storage or inferred region
incomplete proof -> Pop GC
invalid proof -> compiler/verifier incident, never generated code
```

Failing to optimize is acceptable. Reclaiming a possibly live value is not.

## Distinct lifetime concepts

The compiler keeps these concepts separate:

- **value liveness**: control-flow points at which an SSA value may be used;
- **semantic object lifetime**: the interval in which valid Pop operations may
  observe an object's value or identity;
- **borrow lifetime**: the interval in which a non-owning view may be used;
- **storage lifetime**: the interval during which physical storage is reserved;
- **ownership domain**: the activation, task, scheduler, isolated region, or
  shared domain authorized to retain the value;
- **resource lifetime**: the explicit lifecycle of a file, socket, buffer, or
  other external resource.

Value liveness may end before its containing activation. A borrow may end
before the lender's semantic lifetime. Physical storage may be retained longer
than semantic liveness for pooling or reuse, but it may never be reused while a
valid access remains. Resource lifetime continues to use explicit `defer`,
`async defer`, and close operations; memory analysis does not turn resource
cleanup into a hidden destructor or finalizer.

## Storage strategy ladder

The compiler selects the earliest safe level in this ladder:

| Strategy | Suitable values | Reclamation |
| --- | --- | --- |
| `Elided` | constants, unboxed scalars, scalar-replaced aggregates | no storage to reclaim |
| `StaticSlot` | fixed-shape non-escaping values with one proven activation/frame owner | exact lifetime end; physical slot is reused or activation storage is released |
| `ScopedRegion` | one or more allocations with a common proven lifetime and no escaping interior reference | bulk close at the exact region frontier |
| `IsolatedRegion` | a runtime graph with exactly one external owner, including internal aliases or cycles | ownership transfer or whole-region dissolution; local tracing may still reclaim earlier internal garbage |
| `Managed` | values whose end is defined by runtime reachability | Pop GC |
| `Immortal` | verified process-lifetime constants and runtime metadata | process/runtime teardown only |
| `UnmanagedResource` | external ABI storage and operating-system/native resources | explicit typed close/cleanup, never GC correctness |

`Elided`, `StaticSlot`, `ScopedRegion`, and `Managed` are storage plans, not
source types. `IsolatedRegion` is also a runtime ownership domain. A class does
not imply GC, a record does not imply stack storage, and an array does not imply
either one. The exact allocation use determines the plan.

### Scalars and scalar replacement

Unboxed primitive values normally require no managed allocation. Tuples,
records, optionals, unions, and fixed arrays may also disappear into SSA values
when the compiler can preserve:

- exact field/element values;
- bounds traps and evaluation order;
- observable class/reference identity where applicable;
- managed-reference liveness at safe points; and
- debug reconstruction required by the selected build mode.

Scalar replacement is the preferred outcome because it removes allocation and
reclamation together.

### Static slots

A `StaticSlot` belongs to one activation owner. That owner may be a native
stack frame, a VM frame, or a compiler-created coroutine frame; canonical MIR
does not prescribe the physical layout.

An allocation is eligible only when the compiler proves that:

- every alias is known and remains within the owner;
- no alias is returned, stored in longer-lived storage, published, sent,
  retained by a handle/callback, or passed to a call that may retain it;
- all identity comparisons while live retain their ordinary result;
- every borrow ends before the slot lifetime ends;
- every managed reference stored in the slot is present in precise root maps
  while live; and
- the slot's lifetime-end frontier covers every normal, result-failure,
  loop-control, unwind, cancellation, and supported suspension path.

Dynamic-size arrays may use activation-owned native storage when their size is
checked and the target provides bounded allocation with deterministic failure.
`StaticSlot` does not mean a machine stack `alloca`; a backend may use an
activation-owned side buffer while preserving the same plan and exact close.

### Compiler-inferred scoped regions

A `ScopedRegion` groups allocations with one proven lifetime. It uses bump
allocation and one failure-atomic bulk close.

The initial region proof requires:

- one `RegionId` and one owning activation;
- every interior alias to end before `regionClose`;
- no managed object, global, task, actor mailbox, callback, or foreign handle
  to retain an interior region reference;
- same-region edges or edges from the region to longer-lived managed/shared
  values only;
- exact typed root maps for every outward managed edge;
- no transfer, publication, pin, or suspension unless the complete region is
  stored in and moves with the owning coroutine frame; and
- one close on every applicable exit after the last interior use.

Cycles and arbitrary aliasing are safe *inside* a scoped region because the
entire region is reclaimed together. They stop being statically reclaimable
when an interior reference may outlive that common frontier.

The first implementation forbids direct pointers between separate scoped
regions. A later region-order proof may permit an edge only when the target
region is proven to outlive the source on every path.

### Isolated regions are not automatically static

An isolated region proves one external owner, not that every contained object
dies at the same time. Whole-region dissolution is exact when the owner ends,
and internal cycles cause no leak at that point. A long-lived isolated region
may still accumulate unreachable intermediate graphs before dissolution.

The runtime may therefore combine:

- static slots and scoped subregions for short handler/request work;
- bulk dissolution for state whose lifetime equals the isolated owner; and
- scheduler-local tracing for internal objects with shorter, runtime-dependent
  lifetimes.

Ownership and reclamation strategy remain distinct facts.

## Where tracing remains necessary

Tracing remains the required correctness mechanism when live storage is
defined by runtime reachability and no enclosing exact-lifetime region owns the
complete graph. Important cases include:

- mutable cyclic graphs that can become unreachable while their activation,
  actor, scheduler, or process continues;
- values with multiple independent retaining aliases whose final release
  depends on runtime input or control flow not captured by a closed summary;
- shared mutable graphs and concurrent structures whose edges change across
  schedulers;
- escaping closure environments and capture cells;
- coroutine/task frames retained by schedulers, task owners, waiters, or
  cancellation state beyond an ordinary call activation;
- values retained by channels, actor mailboxes, registries, caches, module
  roots, strong handles, or owned callbacks;
- graphs crossing an opaque unsafe or foreign boundary through an accepted
  long-lived handle; and
- any allocation for which analysis is unavailable, exceeds its deterministic
  budget, or cannot reconstruct a complete proof.

These are graph/usage properties, not a hard-coded list of GC-only source
types. An array local to one calculation may be static; the same `Array<T>`
stored in shared state is managed. A non-capturing closure may be a code
reference; an escaping mutable closure environment is managed.

### Actors

Actors are deliberately mixed rather than declared universally GC-only:

| Actor memory | Preferred strategy |
| --- | --- |
| scalar handler locals | `Elided` or `StaticSlot` |
| temporary decode/command graph with one handler lifetime | `ScopedRegion` |
| copied message retained in a bounded mailbox | managed or mailbox-owned storage until dequeue |
| state whose whole lifetime equals one actor incarnation | isolated region |
| intermediate cyclic state discarded while the actor continues | scheduler-local tracing |
| shared registry/reference metadata | shared managed heap |

An actor can therefore reduce GC pressure substantially, but a long-lived
actor with runtime-mutated internal aliases cannot use incarnation-wide bulk
reclamation alone without retaining dead intermediate state.

## No universal reference-counting fallback

Pop does not insert ordinary per-object reference counts after a static proof
fails.

Reference counting can provide prompt reclamation, and a future Pop GC
implementation may use counting internally as one collector technique.
However, making it the language-wide fallback would add increments/decrements,
atomic shared costs, recursive release work, and a separate cycle-collection
contract. It would also make storage strategy depend on source-visible weak or
unowned edges, which are intentionally absent from the first release.

The accepted fallback remains precise tracing. Any future counting/tracing
runtime hybrid must preserve the same MIR, reachability, identity, root,
barrier, and latency contracts through its own ADR.

## Source-language contract

Static reclamation introduces no new source syntax in the first release.

In particular, Pop does not add:

- lifetime parameters or apostrophe lifetime names;
- `move`, `borrow`, `owned`, `box`, `delete`, or `free` expressions;
- source-visible stack/arena/heap allocation selectors;
- implicit destructors or RAII-style user code on memory release;
- weak/unowned references to make a compiler proof succeed; or
- a compilation error merely because an allocation must use GC.

The normal experience is "Rust-like proof where inferred, GC where needed"
without Rust-shaped surface syntax. Existing explicit lifetime constructs keep
their own semantics: `defer`, `async defer`, `Ffi.withPin`, `Ffi.Buffer<T>`,
`Ffi.Handle<T>`, structured task groups, actor ownership, and isolated-region
transfers are not rewritten into hidden memory destructors.

Future annotations may request diagnostics or performance constraints, but
they cannot become necessary for ordinary memory safety without a separate
language decision.

## Analysis contract

Storage planning runs after typed HIR, closure/coroutine planning, and initial
canonical MIR verification. It consumes only compiler-proven facts.

### Allocation and alias graph

Each allocation receives one stable `AllocationSiteId`. Analysis records:

- exact semantic type and object/reference map;
- all SSA aliases and block-argument flows;
- field, element, capture, and collection stores;
- identity tests and interior borrows;
- calls and their closed lifetime summaries;
- returns, globals, module roots, handles, callbacks, channels, tasks, actors,
  publication, isolation, and FFI transitions;
- safe points, suspension, cleanup, unwind, cancellation, and trap exits; and
- the ownership domain at every relevant program point.

An unmodelled operation is retaining. It never means "probably safe."

### Closed call lifetime summaries

Effects alone do not state whether a callee retains an argument. ADR 0097
therefore separates parameter retention from result provenance:

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

The structured summary is inferred from typed bodies, checked at
overrides/interface implementations, and emitted in public reference metadata
and portable generic specialization capsules. `MayRetain`/`MayAlias` are
conservative static facts, not dynamic operations. Missing, incompatible, or
budget-exhausted metadata selects both conservative facts. That forces ordinary
owned allocations toward managed storage but rejects a borrowed view rather
than creating a managed/runtime borrow fallback.

A function-value type carries the closed summary required by all possible
callees. An interface implementation may not retain where its interface member
promises `DoesNotRetain`, and a view result must preserve the declared exact
source-parameter provenance.

### Path-sensitive lifetime frontier

The lifetime frontier is the set of control-flow edges after which no valid use
or borrow can occur. It need not be one lexical block end. A conditional value
may have different frontiers on different paths.

The analysis computes the frontier from:

- backwards SSA/reference liveness;
- alias closure;
- post-dominance where one common end exists;
- edge-specific ends where it does not;
- cleanup and unwind/cancellation reachability; and
- coroutine spill liveness across each suspension.

The frontier may move earlier only when memory reclamation has no user-visible
callback. It may never cross a use, root obligation, borrow, publication, or
cleanup body that observes the value.

### Partial escape

When an allocation escapes on only some paths, the optimizer may:

- sink a managed allocation into the escaping path;
- keep a static representation on non-escaping paths and materialize one fully
  initialized managed object before the first escape; or
- conservatively keep the original allocation managed.

Materialization is legal only before an external alias can distinguish the
representations, and it must preserve identity among every live alias. The
first implementation may choose the conservative managed plan. It must not
copy an already published identity merely to recover a static optimization.

## HIR and MIR contract

Typed HIR retains allocation origins, captures, ownership transitions,
suspension plans, and closed lifetime summaries. It does not choose a physical
stack address or collector representation.

Canonical construction MIR begins with ordinary managed-capable allocations.
After it verifies, the portable storage-planning pass may produce optimized
canonical MIR with these backend-neutral identities and operations:

```text
AllocationSiteId
LifetimeId{kind = Storage | Borrow}
RegionId
StoragePlan = Elided
            | StaticSlot{lifetime}
            | ScopedRegion{region, lifetime}
            | Managed{allocationClass}
            | Immortal

lifetimeStart{lifetime, allocationSite, proof}
lifetimeEnd{lifetime}
regionOpen{region, lifetime, layoutBudget}
allocateInRegion{region, allocationSite, objectMap}
regionClose{region}
```

`AllocationSiteId` names one managed-capable construction use and remains
traceable after optimization. A storage-kind `LifetimeId` governs a slot or
region frontier; a borrow-kind `LifetimeId` governs one view. `RegionId` names
only a compiler-inferred storage region and remains distinct from ADR 0087's
foreign `BorrowRegionId`. A non-allocating `Text.View`/`Bytes.View` descriptor
has no allocation site or storage plan of its own; it records its lender and a
contained borrow lifetime under ADR 0097.

The closed initial `StaticReclamationProof` kinds are:

- `NonEscapingAllocation`: one allocation and all aliases remain inside one
  activation owner; and
- `CommonLifetimeRegion`: every member allocation and interior alias shares one
  verified region frontier.

Proofs are reviewable compiler facts, not forgeable source annotations. The
verifier reconstructs their safety from MIR; a backend does not trust a flag or
repeat escape analysis with weaker rules.

The MIR verifier requires:

- every `LifetimeId`, `RegionId`, and allocation membership to be unique and
  well typed;
- start/open to dominate all uses and end/close to post-dominate or cover every
  applicable exit;
- no use, borrow, root, cleanup observation, or outward edge after end/close;
- no static/region reference in a managed, shared, global, handle, mailbox,
  callback, or foreign-retained slot;
- exact object maps and safe-point roots for managed references contained in
  static storage;
- balanced region close on normal, result-failure, unwind, cancellation, and
  supported coroutine-destruction paths;
- preservation of traps, bounds, evaluation order, identity, and initialization;
- no barrier on purely static/region-internal edges and ordinary barriers for
  every managed edge that still requires one; and
- re-verification after every transformation.

Construction MIR, optimized MIR, the MIR interpreter, LLVM, and a future VM
must agree on the plan. This replaces backend-only guesses such as freeing one
special scalar-array shape only on a normal return.

## Runtime and PLRI contract

`Elided` and fixed `StaticSlot` storage normally require no PLRI allocation.
Their managed children remain ordinary precise roots until `lifetimeEnd`.

`ScopedRegion` uses backend-neutral PLRI operations for:

- bounded region open;
- typed aligned bump allocation;
- precise outward managed-root registration/update;
- deterministic capacity and memory-limit failure;
- failure-atomic close and root release; and
- telemetry.

PLRI does not expose a compiler arena object, raw address, LLVM `alloca`, or C
`malloc`/`free` spelling. A native backend may lower a static dynamic-size
array to checked activation-owned native storage, while a VM may use a frame
side buffer. Both consume the same lifetime plan and close frontier.

Static storage counts toward the runtime hard memory limit and emergency
headroom. The artifact records the optimized storage plan, so one build does
not silently switch between managed and static allocation at runtime.

## GC interaction

Static memory may contain managed references but is never conservatively
scanned.

- live static slots appear in exact stack/frame maps;
- scoped regions publish exact outward root slots;
- a moving collection updates those slots before execution continues;
- `lifetimeEnd` and `regionClose` remove the corresponding roots atomically;
- managed objects cannot point into static/scoped storage; and
- static storage cannot be pinned or turned into an `Ffi.Handle<T>` target.

If a value must become a managed target, the compiler selects/materializes a
managed representation before the transition. No runtime searches native
stacks or arena bytes for a hidden reference.

## Identity, mutation, and barriers

Reference identity remains semantic while the value is live. A static class or
array may use a backend-private activation identity, but every alias and
identity comparison must observe the same identity. Identity cannot escape its
storage lifetime.

Stores into unpublished static slots or one scoped region need no GC write
barrier. Stores of managed references into static storage update precise roots.
Stores from managed storage to static/region storage are forbidden. A managed
store remains governed by ADR 0080's verified barrier proof contract; static
reclamation does not weaken it.

Mutation never extends a lifetime implicitly. If a mutation creates an alias
that may escape the current plan, the plan must already be managed or the
compiler must materialize managed storage before the mutation becomes visible.

## Closures, tasks, and suspension

- A non-capturing function is a typed code reference and allocates no
  environment.
- A non-escaping read-only closure environment may be scalar-replaced or use a
  static slot.
- A closure passed to a `DoesNotRetain` call may borrow activation-owned
  captures for that call.
- An escaping closure or shared mutable capture cell is managed unless a whole
  enclosing scoped-region proof closes all aliases together.
- A value live across `await` belongs to the compiler-created coroutine frame,
  not the abandoned native stack. It may use a frame `StaticSlot` only when the
  exact spill and task-destruction frontier is verified.
- A scheduler-retained task frame remains a precise managed/root container
  under ADR 0077 and ADR 0079. Static analysis may remove allocations *inside*
  it without pretending the runtime-determined task lifetime is a lexical
  native-stack lifetime.

Work stealing cannot leave a pointer into another worker's native activation
storage. Frame-owned static storage migrates only with the complete frame.
Scoped scheduler-local regions either migrate through an accepted ownership
transition or keep the task scheduler-affine.

## FFI and resources

A pointer into a `StaticSlot` or `ScopedRegion` is not a foreign pointer.
Ordinary FFI cannot retain it, and a GC handle cannot name it.

The existing safe paths remain:

- `Ffi.withPin` for one non-escaping immutable `Bytes` payload borrow;
- `Ffi.Buffer<T>` for explicitly closed unmanaged ABI storage;
- `Ffi.Handle<T>` for a long-lived managed value; and
- copied/marshalled values for fixed foreign layouts.

Choosing `Ffi.Handle.open(value)` forces `value` to have a managed
representation before the call. A scoped region may hold a handle token as a
scalar/resource fact only while its ordinary explicit close remains verified;
region close does not invent handle cleanup.

## Separate compilation and optimization stability

Public reference metadata and generic specialization capsules carry closed
lifetime summaries, not bodies or compiler pointers. The summary is part of
the compatibility fingerprint because changing `DoesNotRetain` to `MayRetain`,
changing `ReturnsAlias`'s source parameter, or weakening `Independent` to
`MayAlias` can invalidate a consumer's storage or view plan.

The compiler may always replace a stronger summary with conservative
`MayRetain`/`MayAlias` and use GC for ordinary owned allocations. A borrowed
view then becomes invalid source rather than a runtime-managed borrow. The
compiler may not reuse cached static-reclamation MIR
after the callee summary, effect summary, target storage capability, object
layout, or compiler proof version changes.

Optimization levels may change how many allocations are proven static, but
never accepted source behavior. Debug builds may retain storage or materialize
debug views longer physically; valid program access still ends at the same
semantic frontier.

## Diagnostics and tooling

Needing GC is not a warning by default. It is normal Pop Lang behavior.

Opt-in optimization remarks may explain:

- `scalar replaced`;
- `activation-owned`;
- `scoped region`;
- `managed because returned/captured/published/suspended/retained`;
- `managed because lifetime summary is conservative`; or
- `managed because analysis budget was exhausted`.

Remarks use stable allocation-site identities and typed reasons. They never
suggest unsafe casts, manual `free`, dynamic lookup, weak references, or
source-level lifetime punctuation as an automatic fix.

MIR dumps show storage plans, lifetime/region identities, proof kinds, and
frontiers. Machine tooling consumes structured plan metadata rather than
scraping explanations.

## Observability and performance gates

Runtime/compiler telemetry records at least:

- allocation sites by selected storage plan;
- bytes and objects elided, activation-owned, region-owned, and managed;
- dynamic-size static allocations and exact closes;
- region opens, peak bytes, closes, and bulk-reclaimed bytes;
- managed roots held by static slots/regions;
- materializations caused by partial escape;
- proof rejection reasons and deterministic analysis-budget exhaustion;
- GC allocation/collection work avoided; and
- compile-time cost of lifetime/escape/region analysis.

Initial gates require:

- unboxed scalars and scalar-replaced values perform no GC allocation;
- proven non-escaping scalar arrays use verified static storage and close on
  every applicable exit;
- escaping or managed-element array negatives retain the managed path until a
  stronger proof exists;
- scoped-region graphs bulk-close without a trace and preserve precise outward
  roots under forced relocation;
- no managed-to-static edge, use-after-end, missed unwind/cancellation close,
  or stale root passes verification;
- interpreter and LLVM produce identical results/traps for every plan; and
- benchmarks report workload, compiler/proof version, optimization mode,
  backend, runtime profile, managed bytes avoided, compile-time cost, peak
  memory, throughput, and latency percentiles.

Static reclamation must improve a paired memory/throughput or latency workload
without weakening correctness. A microbenchmark showing fewer GC calls is not
enough to claim broad performance.

## Implementation sequence

1. Move the existing non-escaping scalar-array decision out of LLVM-only
   inference into a portable MIR analysis and proof.
2. Add `AllocationSiteId`, `LifetimeId`, closed call lifetime summaries, and
   verifier-negative fixtures.
3. Support `Elided` and fixed/dynamic `StaticSlot` lowering in the MIR
   interpreter and LLVM, including every exit frontier.
4. Connect compiler-generated `ScopedRegion` operations to the existing typed
   collector arena machinery and precise root updates.
5. Add path-sensitive partial escape and managed materialization only after the
   conservative whole-allocation plans pass differential and forced-GC tests.
6. Extend closure and coroutine-frame allocation elimination without weakening
   task-root, cancellation, migration, or FFI contracts.
7. Optimize region nesting, slot reuse, and allocation fusion from measured
   evidence.

Each stage follows architecture, failing deterministic tests, minimal
implementation, and cross-backend verification. The experimental C backend may
reject plans outside its declared runtime-free subset and is not a parity
priority.

## Required conformance matrix

Positive coverage includes:

- scalars, tuples, records, fixed arrays, dynamic scalar arrays, classes with
  local identity, and non-escaping closure environments;
- branch-specific final use, loops, early return, result failure, cleanup,
  unwind, cancellation, and coroutine-frame storage;
- same-region cycles, outward managed references, nested allocation, and bulk
  close;
- separate-Bubble `DoesNotRetain` calls and generic specialization; and
- local Text/Bytes views, re-slicing, exact parameter-alias returns, and
  explicit owned materialization under ADR 0097.

Negative coverage includes:

- return, capture, global/module store, managed-field store, handle/callback
  retention, channel/send/mailbox publication, actor escape, FFI retention, and
  unsupported suspension;
- managed element/reference maps when roots cannot be represented exactly;
- forged proof, duplicate/missing end, end before use, missing exit, cross-region
  pointer, managed-to-region pointer, stale metadata, and wrong call summary;
- identity-changing materialization and allocation before a conditional escape
  that changes evaluation/failure order;
- analysis timeout/budget exhaustion falling back to managed allocation; and
- view aggregate/store/capture/suspension/ownership/FFI escape and missing,
  conservative, or wrong result-provenance rejection without runtime fallback.

Regression coverage permanently proves:

- no source ownership/lifetime syntax is required;
- no explicit `free`, implicit destructor, user finalizer, reference-count
  field, weak edge, conservative scan, or dynamic fallback is introduced;
- HIR/MIR contain no LLVM stack/address operations;
- backends do not invent independent weaker escape rules; and
- GC remains available for cycles, shared aliases, and runtime-retained graphs.

## Explicit non-goals

- proving every allocation static;
- replacing Pop GC or weakening its production requirements;
- making allocation placement part of a source type's identity;
- exposing Rust, Cyclone, C++, or linear-type syntax;
- deterministic user destructor execution at last use;
- using reference counting as a hidden universal fallback;
- reclaiming external resources from memory liveness;
- allowing raw pointers into stack, frame, or region storage to escape;
- optimizing the experimental C backend to parity; or
- treating actors, classes, arrays, closures, or records as uniformly GC or
  uniformly static.

## Summary

Pop Lang uses a proof-directed hybrid:

```text
SSA/register or scalar replacement
    -> activation-owned static slot
        -> compiler-inferred scoped region
            -> isolated owner/region
                -> precise tracing GC when runtime reachability decides death
```

The compiler is exact whenever it chooses static reclamation and conservative
whenever proof is incomplete. This preserves Pop Lang's small Luau-shaped
surface while giving simple scalars, arrays, aggregates, and temporary graphs
the same practical benefit as explicit ownership languages: they avoid GC when
the compiler already knows their complete lifetime.
