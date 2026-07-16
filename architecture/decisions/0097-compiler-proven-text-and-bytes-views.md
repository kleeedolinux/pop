# ADR 0097: Compiler-Proven Text and Bytes Views

- Status: accepted
- Date: 2026-07-16
- Supersedes: none
- Extends: ADR 0032, ADR 0055, ADR 0068, ADR 0082, ADR 0085, and ADR 0087

## Context

The public-library architecture promises non-allocating `Text.View` and
`Bytes.View` values, but it did not define the accepted first-release
operations or the proof that prevents a view from outliving its storage. ADR
0085 supplies allocation, lifetime, region, storage-plan, and conservative
retention facts,
but its retention-summary list combines parameter retention with result
provenance and is not precise enough for a separately compiled view-returning
function.

Leaving that boundary implicit would force one of three unacceptable outcomes:
a runtime borrow table, source lifetime syntax selected without design, or
backend-specific escape rules. It would also leave `Iterator<Text.View>`, view
fields, suspension, native calls, and missing `.poplib` metadata with no
fail-closed answer.

The first release needs a deliberately narrow useful surface. It must preserve
Pop Lang's ordinary managed ergonomics for owned values while making every
borrowed use a compiler proof rather than a runtime guess.

## Decision

### Owned storage and borrowed views are distinct

`String` is an immutable owned valid-UTF-8 value. `Bytes` is an immutable owned
byte sequence. Mutable construction and reuse belong to the separate
`Bytes.Buffer` value; the first view surface cannot borrow mutable buffer
storage.

`Text.View` and `Bytes.View` are compiler-known nominal non-owning value kinds.
They are not classes, records, interfaces, pointers, handles, resources, or
runtime-reflection objects. A view designates one immutable lender plus a
checked byte range. A `Text.View` additionally proves that both range endpoints
are UTF-8 scalar boundaries.

A view operation allocates no storage. A view value is a non-allocating MIR
descriptor and therefore has no `AllocationSiteId` or independent
`StoragePlan`. Its lender keeps its own plan: `Elided`, `StaticSlot`,
`ScopedRegion`, `Managed`, or the narrow accepted `Immortal` plan. The compiler
keeps the lender live and precisely rooted until the view's last permitted use.
It never promotes a failed view proof into a managed view or runtime borrow.

### Accepted first-release API contract

The accepted first-release surface is exactly:

```luau
public function Bytes.view(bytes: Bytes): Bytes.View
public function Bytes.slice(bytes: Bytes, start: Int, length: Int): Bytes.View
public function Bytes.slice(view: Bytes.View, start: Int, length: Int): Bytes.View
public function Bytes.length(view: Bytes.View): Int
public function Bytes.get(view: Bytes.View, index: Int): Byte?
public function Bytes.toBytes(view: Bytes.View): Bytes

public function Text.view(text: String): Text.View
public function Text.slice(text: String, start: Int, length: Int): Text.View
public function Text.slice(view: Text.View, start: Int, length: Int): Text.View
public function Text.length(view: Text.View): Int
public function Text.toString(view: Text.View): String
```

Indexes are one-based. `Bytes.slice` measures `start` and `length` in bytes.
`Text.slice` measures them in Unicode scalar values, never bytes or grapheme
clusters. `Text.length` returns the scalar-value count. For either slice, a
zero length is valid at any boundary from `1` through `ownerLength + 1`.
Otherwise `start` must be in `1..ownerLength`, `length` must be nonnegative,
and checked addition must prove the selected range ends within the input.
Invalid bounds raise the existing closed `BoundsViolation` trap before a view
is produced. Re-slicing is relative to the supplied view and keeps the original
lender.

`view`, `length`, and `Bytes.get` allocate nothing; `Bytes.get` returns absence
for an out-of-range index. `slice` allocates nothing and may trap as specified.
`toBytes` and `toString` explicitly materialize new owned storage and may
allocate and reach a safe point. They copy only the selected range. The empty
result follows the ordinary canonical empty-owned-value behavior.

The API summary facts are exact. Every parameter is `DoesNotRetain`.
`Bytes.view`, both `Bytes.slice` overloads, `Text.view`, and both `Text.slice`
overloads return `ReturnsAlias` of their `bytes`, `view`, or `text` lender
parameter. `length`, `get`, `toBytes`, and `toString` return `Independent`.

`Bytes.view`, `Bytes.slice`, `Bytes.length`, and `Bytes.get` are O(1).
`Bytes.toBytes` is O(n) in selected bytes. `Text.view` is O(1);
`Text.slice` and `Text.length` are O(n) in UTF-8 bytes inspected; and
`Text.toString` is O(n) in selected bytes. Implementations may retain an
unobservable scalar-count cache or share canonical empty/full-range immutable
storage, but cannot worsen these bounds or expose sharing as identity.

The canonical ADR 0022 effect summaries are exact: `view`, `length`, and
`Bytes.get` have none of allocation, mutation, trap, panic/unwind, suspension,
unsafe memory, FFI, ambient I/O, or safe-point effects. `slice` adds only the
possible `BoundsViolation` trap. `toBytes`/`toString` have the ordinary owned
allocation and safe-point effects and no mutation, suspension, unsafe-memory,
FFI, or ambient-I/O effect. Retention/provenance remains the separate structured
summary below; it is not folded into the effect set.

Word, grapheme, line, token, regex-capture, mutable-buffer, memory-mapped, and
iterator-yielding views are not stabilized by this ADR. In particular,
`Iterator<Text.View>` is not part of the first-release contract. Each later
family must preserve or deliberately extend this proof model through another
accepted decision.

### Compiler lifetime identities

The compiler assigns identities before storage planning:

- `AllocationSiteId` names one managed-capable construction-MIR allocation use.
  It is deterministic within the Bubble artifact and remains attached through
  optimization even when storage is elided or made static.
- `LifetimeId` names one verified non-lexical interval and carries the closed
  kind `Storage` or `Borrow`. A `StaticSlot` refers only to a storage lifetime;
  each produced or re-sliced view refers to one borrow lifetime.
- `RegionId` names one compiler-inferred `ScopedRegion` storage owner and its
  exact open/close frontier. It never doubles as a view-borrow identity.
- `BorrowRegionId` remains the distinct lexical foreign-pointer proof from ADR
  0087. A `Text.View` or `Bytes.View` does not create an FFI borrow region.

Each borrow lifetime records one `ViewLender`: an allocation site, a parameter
provenance slot, or an immutable constant. Re-slicing records the parent view
and original lender. Its frontier must be contained in both the parent borrow
and lender storage/semantic lifetime. Dense session IDs are never public
identity; serialized implementation facts pair them with the owning Bubble,
origin fingerprint, and proof-schema version.

### Structured callable lifetime summaries

ADR 0085's flat lifetime-summary vocabulary is normalized into one closed
`CallableLifetimeSummary`:

```text
parameterRetention[i] =
    DoesNotRetain
  | MayRetain
  | StoresInto(targetParameter)
  | Captures
  | Publishes

resultProvenance[j] =
    Independent
  | ReturnsAlias(sourceParameter)
  | MayAlias
```

`StoresInto(targetParameter)` means an alias derived from parameter `i` may be
stored in storage reachable through the named target parameter. `Captures`
means a closure or callable environment may retain it. `Publishes` includes a
global, task, channel, actor, callback, handle, foreign, or other externally
retained destination. These three facts are stronger reasons within
`MayRetain`, not permission to pass a view.

`Independent` proves that the result has no borrow derived from any argument.
`ReturnsAlias(sourceParameter)` proves the exact source of a borrowed result.
`MayAlias` is the conservative result fact. A summary can mix facts for
different parameters and result-pack positions; it is not a single function
effect.

Missing, malformed, version-incompatible, or unverified metadata decodes to
`MayRetain` and `MayAlias` for ordinary allocation planning. It does not permit
a view argument or view result. Public or indirect callable types containing a
view require the exact usable facts and otherwise fail statically.

A view may be passed only in a parameter position whose fact is
`DoesNotRetain`. A returned view requires `ReturnsAlias(sourceParameter)`, and
the caller substitutes the actual lender provenance for that parameter. A
function may return a view of an input `String`, `Bytes`, or view parameter;
it may not return a view of a local owned value, local region, or temporary
whose lender cannot be transferred through an exact parameter provenance.

No `LifetimeId` crosses a call or Bubble boundary. The callee's result borrow
ends at its return frontier after producing the checked range. The caller uses
`ReturnsAlias(sourceParameter)` to create a fresh borrow `LifetimeId` over its
actual lender and the returned range. This rebinding is static provenance
substitution, not a runtime borrow, copy, or lifetime token.

The summary is inferred and verified from every callable body. It has no source
attribute or lifetime punctuation. Function values and interface/virtual calls
carry the same closed summary in their static callable type or slot contract;
there is no runtime retention query or optimistic indirect-call rule.

### First-release containment and escape rules

A view may be a local, a parameter, or one direct result governed by the exact
summary above. Direct local assignment and branch joins preserve the same
lender provenance.

The first release rejects a view in:

- a class field, closure/capture cell, Module state, global, static slot owned
  by another semantic value, or managed/scoped-region allocation;
- an array, list, table, record, tuple, union, optional, result, iterator state,
  generic collection, or another aggregate;
- a returned result without exact `ReturnsAlias` provenance;
- a call position that is `MayRetain`, `StoresInto`, `Captures`, `Publishes`, or
  missing metadata;
- a coroutine frame across `await` or another `Suspends` operation;
- a task, channel, actor message, callback environment, resource, handle, or
  isolated/shared ownership transfer; or
- a foreign parameter/result, `Ffi.Buffer`, `Ffi.Handle`, `Ffi.withPin`, raw
  pointer conversion, generated binding, or native callback.

A view can be created and consumed inside an async function only when its
complete borrow lifetime ends before the next suspension or task publication.
Capturing a view is rejected even when the closure appears local; this keeps the
first proof independent of closure escape and delayed cleanup analysis.

FFI callers materialize `Bytes`/`String`, copy into `Ffi.Buffer<T>`, or pin an
owned immutable `Bytes` value through the existing immediate scoped API. A
view never exposes a payload address, pins its lender, or becomes a handle
target.

These are source rejection rules, not reasons to select `Managed` for the view.
The lender itself may remain managed when ordinary allocation proof is
conservative.

### HIR and canonical MIR

HIR retains `ViewCreate`, `ViewSlice`, and `ViewMaterialize` expressions with
the exact view kind, lender provenance, checked range unit, result type,
`LifetimeId`, and origin. Calls retain the structured lifetime summary after
resolution and specialization.

Canonical MIR uses backend-neutral operations:

```text
viewCreate{kind, lender, borrowLifetime}
viewSlice{kind, view, start, length, borrowLifetime, boundsTrap}
viewLength{kind, view}
viewGetByte{view, index}
viewMaterialize{kind, view, allocationSite}
viewEnd{borrowLifetime}
```

A MIR view value contains the typed lender SSA value and checked byte
offset/length; `Text.View` also retains the scalar-length/boundary facts needed
to verify scalar-relative slicing. It does not contain an exposed raw interior
pointer. `viewEnd` occurs on every frontier after the final use, including
normal, result-failure, unwind, cancellation, and pre-suspension exits.

The verifier proves:

- exact kind, lender type, range unit, bounds trap, and UTF-8 boundaries;
- one unique borrow `LifetimeId`, dominance of creation, and every-exit end;
- containment within parent borrow and lender lifetime;
- no aggregate/store/capture/suspension/ownership/FFI escape;
- call arguments and results agree with the exact structured summary;
- a managed lender remains in the precise mutable root set at every safe point;
- no cached interior address survives a relocating safe point; and
- transformations preserve provenance, range, summary, and frontier facts.

Invalid or incomplete MIR is a verifier incident. A backend cannot insert a
runtime borrow check, extend a lifetime, copy silently, or reinterpret the view
as a pointer.

### Runtime, interpreter, LLVM, and experimental C

PLRI gains no general borrow manager. Owned `String`/`Bytes` allocation and
materialization use their ordinary typed runtime operations. Bounds and UTF-8
facts remain canonical MIR semantics.

The MIR interpreter represents a view as an internal typed lender plus range
and enforces only the already verified operations. LLVM lowers a managed lender
as a live relocatable root plus offsets and recomputes any ephemeral payload
address after a safe point; static/region lenders use their verified storage
base and the same offsets. Neither backend stores a long-lived raw interior
pointer or maintains a borrow table. Differential tests force lender relocation
between view operations.

The experimental C backend fails capability validation for every view MIR
operation and every callable signature containing a view. Its runtime-free
literal-string output support does not satisfy the owner, relocation, UTF-8
slice, or lifetime-summary contract. It emits no partial C and cannot lower a
view to `char *`, `void *`, or an implicit copy.

### Artifact and compatibility facts

Public `reference.metadata` records view kinds in closed callable signatures
and the complete structured lifetime summary. A public view result must name
one exact source parameter. The summary schema and proof version participate in
the public API/compatibility fingerprint. Weakening `DoesNotRetain`, changing a
`ReturnsAlias` source, or changing an `Independent` result to `MayAlias` is an
incompatible change for consumers that may have formed view proofs.

`AllocationSiteId`, `LifetimeId`, `RegionId`, concrete `StoragePlan`, ranges,
and proof graphs are implementation facts. They remain in verified
implementation/capsule metadata when needed for execution or specialization,
paired with stable origin fingerprints; they do not enter consumer name
resolution, public reflection, or documentation APIs. A cache invalidates on
summary, effect, layout, target capability, or proof-schema mismatch.

Reference-only loading rejects a view signature whose summary is absent,
noncanonical, out of range, or inconsistent with its parameter/result types.
Generic specialization capsules cannot instantiate a view inside a forbidden
aggregate and cannot replace exact provenance with a session-local ID.

### Diagnostics and fixes

The diagnostic catalog reserves:

- `POP2035` for a borrowed view that escapes its lender, with a typed reason
  such as return, store, aggregate, capture, ownership transfer, or FFI;
- `POP2036` for passing a view to a callable position that may retain it;
- `POP2037` for a view live across suspension;
- `POP2038` for a callable view signature without exact usable provenance; and
- `POP7040` for invalid artifact or MIR lifetime-summary/provenance facts.

Diagnostics identify the view origin, lender, attempted destination or call,
and the lifetime/summary fact that rejected it. A `RequiresReview` action may
offer the exact explicit `Bytes.toBytes` or `Text.toString` materialization when
the destination expects that owned type. It must disclose copying/allocation
and is excluded from unattended fix-all. No fix adds a lifetime annotation,
unsafe cast, pin, handle, dynamic wrapper, or hidden copy.

## Consequences

- Local zero-copy text and byte slicing has an accepted release contract without
  source lifetime syntax or runtime borrow state.
- Exact alias returns remain usable across Bubble boundaries.
- The first release deliberately excludes view-bearing containers and lazy
  view iterators; later APIs must earn a wider proof model.
- Managed fallback remains available for owned lenders, while an unproven view
  is a static error rather than a managed borrow.
- Interpreter and LLVM share one relocation-safe representation contract; the
  experimental C backend stays fail-closed.

## Alternatives considered

### Add source lifetime parameters or borrow modifiers

Rejected for the first release because the narrow surface is fully expressible
through inferred provenance and summaries, and premature syntax would reshape
the whole type system.

### Keep an owner reference in every managed view object

Rejected because it would turn a promised non-allocating view into an escaping
runtime object, hide retention, and require runtime rules for mutation,
suspension, and ownership transfer.

### Copy automatically when a view escapes

Rejected because allocation, copying, identity, and failure would become hidden
semantic changes. Materialization is explicit.

### Permit `Iterator<Text.View>` immediately

Rejected because an iterator state must express the relationship among its
owned or borrowed source, each yielded borrow, repeated calls, storage, and
suspension. That needs a separate container/iterator lifetime decision.

### Treat missing summaries as non-retaining

Rejected because old, malformed, or hostile metadata could create a
use-after-lifetime across a Bubble boundary.

## Required conformance tests

- exact API identity, overload, visibility, one-based bounds, zero-length,
  overflow, UTF-8 scalar-boundary, optional byte access, and allocation/copy
  fixtures;
- local view, re-slice, branch-join, direct `DoesNotRetain` call, and exact
  parameter-alias return positives;
- local-owner return, aggregate/container/field/global/store, capture,
  retaining/unknown call, suspension, task/channel/actor/callback/handle, FFI,
  mutable-buffer, and iterator-view negatives;
- artifact round trips for structured summaries plus missing, malformed,
  out-of-range, incompatible-version, wrong-parameter, forbidden-generic, and
  fingerprint-change negatives;
- HIR/MIR identity, dominance, containment, every-exit `viewEnd`, root-map,
  UTF-8, summary, and optimizer-corruption tests;
- MIR-interpreter/LLVM differential execution with forced relocation between
  create, slice, length/get, and materialization;
- C capability rejection with no partial artifact and no pointer/copy fallback;
- diagnostics and `RequiresReview` materialization previews; and
- architecture regressions forbidding runtime borrow tables, source lifetime
  syntax, dynamic escape, hidden copies, raw interior pointers, and the
  stabilization of `Iterator<Text.View>` without a later ADR.

## Documents/components affected

Type and lifetime checking, effect/capture analysis, HIR, MIR, verifier,
optimization, reference metadata, specialization capsules, `Pop.Standard`
Text/Bytes APIs, PLRI adapters for owned materialization, MIR interpreter, LLVM,
experimental C validation, GC root maps, async/actor checks, FFI checks,
diagnostics, documentation, and conformance suites.
