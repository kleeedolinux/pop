# ADR 0054: Inferred, Constrained, and Portable Generics

- Status: accepted
- Date: 2026-07-13
- Depends on: ADR 0001, ADR 0003, ADR 0007, ADR 0020, ADR 0036,
  ADR 0050, and ADR 0053
- Supersedes: the inference, portable-reference, generic-class, and typed-sharing
  deferrals in ADR 0050

## Context

ADR 0050 establishes executable generic functions, records, and tagged unions
through explicit type arguments and full concrete MIR specialization. That
bootstrap cannot express the ordinary `Sequence` algorithms accepted by ADR
0053. Those algorithms require inferred type arguments at concise call sites,
statically proven iteration bounds, generic iterator-state classes, and
specialization of a public generic implementation owned by another Bubble.

Treating `Sequence.map` as a compiler-known name would make an ordinary portable
library algorithm compiler policy. Loading dependency source into a consumer
would collapse Bubble ownership and visibility. Erasing generic types or
shipping an unchecked dictionary would violate Pop Lang's static contract.

## Decision

### Bounds and generic classes

An ordered type parameter may carry one nominal interface bound with a
Luau-shaped colon:

```luau
private function consume<T, TSource: Iterable<T>>(source: TSource)
end

private class MappingIterator<T, U> implements Iterator<U>
end
```

The bound must resolve to one exact nominal user or reserved built-in interface
instance. It may mention earlier type parameters; it cannot mention a later
parameter, itself recursively, a class, a union, an unresolved type, or a
structural shape. A parameter without a bound accepts any statically resolved
type. Multiple or intersection bounds remain outside the first-release
surface.

Functions, records, tagged unions, errors, classes, and interfaces use the same
ordered parameter syntax and invariant arguments. Class fields, receiver/static
method signatures and bodies, constructor-style static functions, and
`implements` entries may use the owning class parameters. A concrete generic
class instance has one specialized nominal identity, field layout, method set,
and exact interface witness mapping. Shape never implies implementation.

Generic code may select members or generalized iteration through a type
parameter only when its bound proves the exact member/interface operation.
Bound dispatch is retained as a resolved interface identity and stable slot; it
never becomes a runtime name lookup.

### Type-argument inference

A normal call to a directly resolved generic namespace function, static method,
or tagged-union/error case may omit all type arguments. Explicit double-angle
calls remain supported. Partial explicit type-argument lists are not supported.

For an inferred call, the checker creates one fresh variable per declared type
parameter and gathers constraints from, in order:

1. the expected result type when one exists;
2. each argument against its declared parameter type from left to right; and
3. each declared nominal interface bound.

Matching recursively follows exact semantic type constructors. It may use a
statically valid class/built-in-collection-to-interface upcast to infer the
interface arguments and may infer through an exact function signature. Mutable
collections and other invariant constructors never infer by widening.
Ordinary accepted conversions are checked only after the generic solution is
known; they cannot manufacture an otherwise ambiguous solution.

Every parameter must have one unique canonical solution satisfying all bounds.
No solution, conflicting solutions, an ambiguous unconstrained parameter, or a
failed bound produces a static diagnostic that identifies the parameter and
constraint origins. The checker never guesses a default, inserts an internal
top type into HIR, or defers selection to runtime. HIR records the complete
ordered canonical argument list exactly as for an explicit call.

### Portable cross-Bubble specialization

Reference metadata represents generic public signatures with stable ordered
parameter identities, nominal bounds, and a closed recursive type vocabulary.
That vocabulary includes primitives, type parameters, fixed tuples and
functions, arrays, lists, tables, optionals, records/unions/errors/classes,
nominal interfaces, and reserved built-in types. Every nominal reference carries
its owning Bubble-scoped identity or reserved `BuiltinTypeId`; no consumer
reconstructs identity from a name.

A public generic callable that requires consumer specialization publishes one
verified portable specialization capsule. The capsule contains its typed
backend-neutral HIR body and the transitive declarations, functions, class
layouts, methods, interface witnesses, and type graph required to specialize
that body. Non-public dependencies in the capsule retain opaque Bubble-scoped
identities and are not entered into the consumer's name-resolution or public
metadata index. Visibility is not widened and no dependency source Module is
merged into the consumer Bubble.

The logical capsule is part of generic reference metadata. The deterministic
`.poplib` encoding may store its bytes in a separately hashed portable section,
but the reference entry records the schema version and content hash. Loading
verifies ownership, identity uniqueness, visibility closure, types, bounds,
effects, HIR invariants, dependency identities, resource limits, and the hash
before the capsule can participate in specialization. A malformed, unsupported,
cyclic, over-budget, or missing capsule fails closed.

Specialized implementation identity is derived deterministically from the
source `SymbolIdentity` plus canonical type arguments. The consumer may assign
session-local IDs internally, but direct calls, diagnostics, cache keys, and
artifact dependency records preserve the source Bubble identity. Private
capsule symbols cannot collide with or become addressable as consumer symbols.

### Specialization and typed sharing

Full concrete specialization is the required first-release correctness path for
all reachable generic functions, data, classes, methods, witnesses, and
cross-Bubble capsules. Equivalent source identity plus canonical argument lists
deduplicate deterministically. A finite specialization budget rejects recursive
expansion rather than emitting partially erased code.

MIR may later share representation-compatible instances only as a verified
optimization. Shared code must receive a closed typed dictionary/witness value
whose layout, operations, effects, and interface slots are known during MIR
construction. The MIR verifier must prove that the shared representation,
calling convention, GC maps, dispatch, and failure behavior equal those of full
specialization. Sharing cannot change program acceptance, identity, observable
semantics, or diagnostics, and a backend cannot choose it independently.

No typed-sharing implementation is required for `0.1.0`; full specialization is
the accepted implementation and semantic reference. This closes the policy
without permitting erased values, dynamic dictionaries, runtime type arguments,
or string resolution.

## Consequences

- `Sequence` can remain an ordinary generic Pop Standard implementation rather
  than a compiler-recognized algorithm family.
- Common generic calls are concise while ambiguous code still requires an
  annotation or explicit double-angle arguments.
- Generic iterator state has native class identity, precise layout, and nominal
  interface witnesses at every concrete instantiation.
- Dependency source and visibility boundaries remain intact while consumers can
  specialize portable public generic implementations.
- Full specialization may increase code size, but it provides one deterministic
  cross-backend baseline before any sharing optimization.

## Alternatives considered

### Infer an unbounded dynamic source type

Rejected because generalized iteration and member selection require a proven
nominal contract; an unconstrained inference variable cannot authorize runtime
operations.

### Use `where` clauses or Rust-style trait bounds

Rejected because a single colon bound reuses Luau's type-annotation direction
with less punctuation. Multiple/intersection bounds can be designed later if
real APIs require them.

### Make `Sequence` compiler-known

Rejected because ADR 0053 requires ordinary `.pop` algorithms and forbids
compiler recognition of names such as `Sequence.map`.

### Merge dependency HIR or source into the consumer Bubble

Rejected because it would widen private/internal ownership, destabilize
identity, and erase the independent Bubble compilation boundary.

### Require typed dictionary sharing immediately

Rejected because it adds an ABI, representation, and GC proof obligation that
is unnecessary for correctness. Verified full specialization is already a
valid deterministic strategy.

## Required conformance tests

- syntax tests cover colon bounds and ordered generic parameters on all accepted
  declaration kinds, including generic classes and interfaces;
- negative syntax/type tests reject later/self bounds, non-interface bounds,
  wrong arity, failed bounds, and structural lookalikes;
- inference tests cover arguments, expected results, exact callable signatures,
  collection/interface upcasts, conflicts, ambiguity, and explicit/inferred
  equivalence without fallback;
- generic-class tests cover specialized fields, receiver/static methods,
  construction, exact interface witnesses, class identity, and GC maps;
- generic generalized-loop tests prove that only a resolved iterable bound
  authorizes acquisition and stepping;
- metadata tests round-trip every accepted recursive type, ordered bounds,
  source identities, effects, and portable capsule hashes while rejecting
  malformed/private-name exposure and unsupported types;
- cross-Bubble tests specialize and execute a public generic body with private
  transitive helpers without widening visibility or colliding symbol IDs;
- HIR/MIR tests retain canonical arguments, deduplicate equivalent instances,
  enforce the specialization budget, and contain no unresolved parameters,
  erased values, runtime type arguments, or dynamic calls;
- MIR interpreter and LLVM differential tests execute the same local and
  cross-Bubble generic instances; the experimental C backend rejects unsupported
  specialized operations explicitly; and
- a regression test proves that adding an ordinary generic Standard algorithm
  requires no bootstrap function, compiler-name, HIR/MIR operation, or backend
  registry edit.

## Documents/components affected

Syntax/nomenclature, type system, interfaces/classes, reference metadata,
Bubble artifacts, HIR, MIR, specialization, standard libraries, interpreter,
LLVM, experimental C capability validation, diagnostics, caches, and
architecture conformance policy.
