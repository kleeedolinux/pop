# ADR 0091: Checked Interface-to-Class Casts

- Status: accepted
- Date: 2026-07-16
- Depends on: ADR 0001, ADR 0020, ADR 0036, ADR 0051, ADR 0054, and
  ADR 0055
- Supersedes: none

## Context

Pop Lang already requires checked downcasts to name a static target and produce
a typed optional or result. It also retains private runtime type facts for GC,
interface dispatch, checked casts, and stack traces without exposing general
runtime reflection. The first-release architecture did not fix the cast's
source syntax, precise nominal relationship, descendant behavior, portable IR,
artifact facts, diagnostics, or backend capability contract.

Leaving those choices to one checker or backend would permit incompatible
behaviors: an unchecked assertion, name-based lookup, exact-class-only matching,
structural matching, a trap on mismatch, or a backend-specific RTTI operation.
The first stable slice must instead be narrow enough to verify completely and
must preserve Pop Lang's Luau-shaped source character.

## Decision

### Source surface and result

The first stable checked nominal cast converts one non-optional nominal
interface value to one fully resolved named class type. It uses the existing
target-type call shape:

```luau
local fileReader: FileReader? = FileReader(reader)

if local concrete = FileReader(reader) then
    concrete:close()
end
```

The callee must resolve in the type namespace to a class type. A generic class
target includes its complete canonical arguments, for example
`Box<Int>(value)`. This is a compiler-known typed conversion, not a class
constructor call, ordinary overload, runtime type value, or function selected
by a string. Class construction remains the keyed initializer or an explicitly
resolved static function such as `FileReader.new(...)`.

The operand must have exactly one non-optional nominal interface type. The
target must be one fully applied concrete class type whose declaration or base
chain nominally implements that exact specialized interface. A structural
method match is insufficient. The initial checked-cast surface rejects:

- class-to-class downcasts;
- interface-to-interface casts;
- casts to records, unions, primitives, type parameters, unapplied generic
  classes, or internal top/compiler-recovery types;
- optional operands before explicit optional narrowing; and
- a target class with no statically proven implementation of the source
  interface.

Those restrictions keep the first slice independently verifiable. Broadening
the source or target families requires a later ADR and cannot reuse this
operation silently.

The result is exactly `TTarget?`. ADR 0051 already chooses typed optional
absence for common checked-downcast paths, and a failed relationship carries no
domain error value to explain. `Result<TTarget, E>` would invent an error family
for ordinary absence and would conflict with ADR 0052's use of `Result` for
recoverable failures with an exact error type. The optional is consumed through
ordinary optional narrowing, `if local`, `??`, or optional propagation. A
failed cast never traps, panics, unwinds, or returns a dynamically typed value.

### Exact and descendant semantics

An interface value retains the concrete class identity of its original object
alongside its statically resolved interface witness. The cast evaluates the
operand once and succeeds when that concrete class is either:

1. exactly `TTarget`; or
2. a transitive subclass whose specialized base chain contains exactly
   `TTarget`.

Generic class arguments participate in identity invariantly. A descendant of
`Box<String>` does not match `Box<Int>`. A same-spelled class from another
Bubble does not match. Sealed targets therefore admit only the exact class,
while open targets admit exact and descendant instances.

Success preserves the original object identity and produces a statically typed
view of the same managed instance. It does not allocate, clone, reconstruct, or
invoke user code. Failure produces the absent value of `TTarget?`. A private or
internal concrete descendant may satisfy a cast to an accessible public base;
the result exposes only the named target's accessible members and does not
reveal the descendant's identity.

### Identity, visibility, and artifacts

The target is resolved under ordinary Module/Bubble visibility before the cast
is typed. A cast cannot name an inaccessible class, bypass a private boundary,
or discover a class that ordinary type lookup cannot resolve. Public signatures
containing the optional result obey the existing public-surface visibility
rules.

Every cast target carries one stable Bubble-scoped class identity plus its
canonical type arguments. Within one verified arena, typed `ClassId` and
`InterfaceId` keys address those declarations. Across artifacts, caches, and
Bubble boundaries, the stable form is the declaration's `SymbolIdentity` plus
canonical argument identities; loading may remap local typed IDs but must retain
that owner mapping. Runtime matching uses verified descriptor identities and
parent identities, never source names, paths, display strings, hashes used as
names, or runtime symbol lookup. Values from separately loaded isolated
contexts do not acquire equal identity merely because their declarations have
the same spelling; the existing serialized context boundary remains intact.

Public reference metadata carries the public class identity, direct specialized
base identity, open/sealed fact, and exact specialized interface witnesses
needed to validate a consumer cast. The loader verifies those facts against the
owning `BubbleIdentity` before resolution. Private descendant descriptors remain
implementation metadata in the linked program and never enter consumer name
resolution or public reflection. Unsupported, incomplete, inaccessible, or
identity-inconsistent metadata fails closed rather than weakening the cast.

Runtime descriptors retain only the class identity and ancestry facts needed
for allocation, GC, dispatch, and checked operations. No source API enumerates
those descriptors, obtains a type object, or performs a cast from a string.

### HIR and MIR

Typed HIR records one `checkedNominalCast` node containing:

- the evaluated operand;
- the exact source `InterfaceId` and canonical interface arguments;
- the exact target `ClassId` and canonical class arguments; and
- the exact `Optional(TTarget)` result type.

HIR verification rejects a non-interface source, non-class or incomplete
target, an unproven nominal implementation relation, an incorrect optional
result, a missing stable owner identity, or a name/string substitute for an
identity.

Canonical MIR uses the existing `checkedDowncast` operation. It consumes one
managed interface reference and carries the resolved source interface identity,
target class identity, canonical generic arguments, and exact optional result
type. It atomically performs the identity/ancestry test and constructs the
typed optional result. It is not reconstructed from a reflective `typeTest`
followed by an unchecked projection.

MIR verification proves that:

- the operand and source interface type agree exactly;
- the target is a reachable concrete class specialization with a proven exact
  source-interface witness;
- the result is exactly the optional target type;
- all identities retain their owning Bubble and canonical arguments;
- success preserves the operand's managed identity; and
- the operation has no allocation, mutation, suspension, FFI, unsafe-memory,
  trap, panic, or unwind effect.

The operation reads immutable internal type metadata and is not a GC safe point.
If a backend calls a runtime helper, that helper has the same no-allocation,
no-unwind, no-safe-point contract. A backend cannot replace absence with a trap
or expose the physical optional or object representation.

### Backend contract

The MIR interpreter is the semantic reference. Its managed interface value
records the concrete specialized class identity, and `checkedDowncast` walks the
verified semantic base chain to produce the exact optional target while
preserving the object token.

LLVM must lower the same operation from canonical MIR. It may inline a
descriptor-identity/parent-chain test or call a closed internal runtime adapter.
Either path compares stable descriptor identities, preserves a live managed
reference across any implementation call, and returns the canonical optional
presence/payload result. LLVM-specific RTTI, symbol names, and layouts never
enter HIR or MIR.

The experimental C backend's accepted capability set has no managed classes,
interfaces, dispatch metadata, or runtime. It must reject `checkedDowncast`
during capability validation with the existing structured unsupported-target
diagnostic before emitting any C. It cannot lower the operand to `void *`, use a
C cast, or synthesize partial RTTI. The eBPF profile likewise rejects the
missing managed-class runtime contracts. A future VM consumes the same MIR
operation and proves the same identity/ancestry behavior.

### Diagnostics and fixes

The type/conversion diagnostic range reserves these stable errors:

- `POP2032`: the target-type call is not one fully applied named class target;
- `POP2033`: the checked nominal cast operand is not one non-optional nominal
  interface value; and
- `POP2034`: the target class does not nominally implement the exact source
  interface specialization.

Ordinary name resolution and visibility failures retain their existing
`POP1000`-range codes. Existing generic-arity and optional-assignment errors
remain authoritative when they fail before or after the cast boundary.

Each checked-cast diagnostic carries typed source/target identities where they
exist, primary and related spans, and a closed reason argument. Human renderers
may display names, but tools never parse those names to recover the identities.
No automatic fix changes the target, inserts an unsafe assertion, adds
reflection, or silently unwraps an optional. A review action may offer ordinary
optional control only when it can preserve single evaluation and the requested
control-flow shape.

## Consequences

- Checked nominal casts remain explicit, concise, and consistent with Pop's
  existing target-type conversion direction without adding an `as` operator.
- Ordinary cast mismatch is typed absence rather than a trap, panic, dynamic
  value, or invented error object.
- Exact and descendant matching has one stable cross-backend meaning, including
  generic specializations and cross-Bubble identity.
- Private runtime metadata can support the operation without becoming runtime
  reflection.
- The initial cast surface intentionally does not provide arbitrary
  class-to-class, union, structural, type-parameter, or string-selected casts.
- The experimental C backend remains fail-closed instead of gaining an unsafe
  pointer escape hatch.

## Alternatives considered

### Add `value as Target` or `value as? Target`

Rejected because it imports C#/Rust/Swift-style cast punctuation into a
Luau-shaped language and risks conflating checked absence with an unchecked
assertion. The accepted target-type call direction is shorter and already
compiler-known for explicit conversions.

### Return `Result<TTarget, CastError>`

Rejected because a nominal mismatch is ordinary absence with no useful error
payload. ADR 0051 already assigns common checked downcasts to optional flow;
`Result` remains for exact recoverable error families under ADR 0052.

### Trap or panic on mismatch

Rejected because mismatch is an expected branch, not a violated runtime
invariant. A trapping cast would encourage unsafe assumptions and make ordinary
interface narrowing harder to compose.

### Match only the exact runtime class

Rejected because an interface value whose object is a valid subclass must be
usable through an accessible base target. Exact specialized ancestry is stable
and does not require reflection.

### Use structural shape or runtime class names

Rejected because structural behavior would bypass nominal `implements`, while
names would collide across Bubbles/load contexts and introduce runtime lookup.
Both violate Pop Lang's static nominal architecture.

### Accept every source/target nominal family immediately

Rejected because class-to-class, interface-to-interface, union, and constrained
type-parameter casts need separate static-validity and representation rules.
They cannot be inferred from this narrow interface-to-class contract.

## Required conformance tests

- lexer/parser and formatter tests cover non-generic and fully specialized
  target-type calls, nesting, multiline operands, and rejection/recovery of
  `as`-style syntax;
- type tests cover exact-class success, descendant success, sealed exact
  targets, generic specialization identity, the exact optional result, and
  single evaluation;
- negative type tests cover class/interface/record/primitive/type-parameter
  targets, class and optional operands, unapplied generic targets, missing
  nominal implementation, shape-only compatibility, inaccessible targets, and
  wrong arity;
- diagnostics snapshots cover `POP2032` through `POP2034`, stable typed
  arguments/spans, and the absence of an unsafe automatic fix;
- HIR construction and verifier negatives cover exact source/target/result
  identities, canonical arguments, Bubble ownership, and forbidden string or
  dynamic substitutes;
- MIR construction and verifier negatives cover `checkedDowncast`, exact
  optional construction, identity preservation, no duplicated operand, and
  rejection of mismatched witnesses, results, effects, or owner identities;
- reference-metadata round trips cover public targets, specialized bases and
  witnesses, same-spelled classes from different Bubbles, malformed identity
  rejection, and non-discoverability of private target names;
- MIR-interpreter and LLVM differential tests cover exact, descendant, private
  descendant through public base, unrelated implementation failure, generic
  match/mismatch, same-spelled cross-Bubble mismatch, and preserved object
  identity;
- native tests cover managed-reference liveness, no allocation/unwind/safe
  point, moving-GC compatibility, and present-versus-absent optional payloads;
- C and eBPF tests prove deterministic pre-emission capability rejection with
  no pointer cast, partial output, or synthesized RTTI; and
- permanent regressions reject `as`, unchecked projection, runtime type/name
  lookup, structural casting, public descriptor enumeration, `Any`/`Dynamic`,
  and backend-specific HIR/MIR fields.

## Documents/components affected

Language model, syntax/nomenclature, type system, HIR/MIR, runtime metadata,
reference metadata, diagnostics, MIR interpreter, LLVM, C/eBPF capability
validation, formatter, architecture conformance, and cross-backend tests.
