# Type-System Architecture

## Contract

Pop Lang's type system combines Luau-like inference ergonomics with a strict
static acceptance rule:

> A program reaches HIR/MIR only when every operation has a statically valid
> operand, result, member, and dispatch type.

Annotations can be omitted where inference succeeds. Failure to infer produces a
diagnostic. It never inserts a dynamic value or defers member selection to
runtime.

## Semantic type representation

Types are canonical/interned semantic values addressed by `TypeId`. Syntax types
are resolved into this representation before body checking.

Core constructors include:

```text
Primitive(Nil | Boolean | IntegerKind | FloatKind | String)
Singleton(value)
Tuple(elements, optional typed variadic tail)
Function(type parameters, parameters, results, effects)
Record(fields, openness policy)
Array(element)
Table(key, value, table policy)
Class(ClassId, arguments)
Interface(InterfaceId, arguments)
Union(members)
Optional(inner)
TypeParameter(ParameterId)
Opaque(OpaqueId)
Never
Error                 // compiler recovery only
```

`Error` suppresses cascading diagnostics but cannot appear in a valid HIR/MIR
module. An internal top type, if useful to constraint solving, has no member or
call operations and must be eliminated/narrowed before valid HIR.

## Inference strategy

A bidirectional constraint system is recommended:

- expressions synthesize a type when context is absent;
- expected types flow inward for literals, lambdas, records, tables, and generic
  calls;
- annotations create equality/subtype constraints rather than separate checking
  rules;
- inference variables are scoped and cannot escape unsolved;
- generalization occurs only at specified immutable/declaration boundaries;
- mutable locals/fields are not unsafely generalized.

Constraints preserve origin spans and a reason chain. Solver failures can then
explain both conflicting requirements rather than reporting only the final type.

## No dynamic fallback

When constraints remain ambiguous, the checker requests an annotation. It does
not select `any`, guess a runtime member, or insert a reflective call.

Examples requiring diagnostics include:

- an empty collection with no expected element type;
- a function whose parameter is never constrained;
- a union member access not valid for every remaining variant;
- a table write inconsistent with its inferred key/value type;
- an overload set with no unique best statically valid candidate.

## Nil and optional types

Pop Lang retains familiar `nil` syntax. `T?` means `T | nil`, and non-optional
references cannot contain `nil`.

Flow analysis narrows optionals after checks. Mutation, aliasing, closure
capture, calls with relevant effects, and suspension points invalidate facts
that are no longer provable.

An equality comparison with `nil` creates complementary facts for a stable
versioned place. Optional pattern binding in `if local`/`while local` accepts
only `T?`, tests presence rather than truthiness, and introduces an immutable
`T` binding only in the successful body.

For `left ?? right`, `left` must be `T?`, `right` must be assignable to `T`,
and the result is `T`. The right side is lazy. For postfix `operand?`, the
operand must be `T?` and the enclosing function must have one optional result
`U?`; no relation between `T` and `U` is required because the absent edge
returns only `nil`. The continuing expression is `T`. These are closed
compiler-known optional operations, not overloads or carrier reflection. See
ADR 0051.

The exact initialization rules for class fields must ensure no read observes an
uninitialized non-optional value.

## Unions and narrowing

Unions are normalized deterministically: flattened, duplicate-free, and ordered
by stable type identity for hashing/dumps. `Never` disappears from unions;
optional syntax normalizes consistently with `nil`.

Narrowing sources can include:

- `nil` and singleton comparisons;
- nominal type tests;
- tagged-record/class discriminants;
- pattern matching;
- predicates explicitly recognized by the type system.

A narrowing fact is tied to a place/version, not merely a variable name, so a
write cannot leave stale facts alive.

Short-circuit control propagates a fact only along the edge where its predicate
is proven. A fact does not escape a join unless every predecessor proves the
same place version and remaining type.

The first `match` statement accepts one tagged-union scrutinee and one arm for
each resolved `UnionCaseId`. Payload bindings receive the declared case types.
Missing, duplicate, foreign, or arity/type-mismatched cases are errors. There is
no wildcard or guard in version one, so exhaustiveness remains exact.

## Records, arrays, and typed tables

These are distinct semantic types even if optimized layouts overlap.

- A record has a statically known named field set and immutable field bindings;
  `with` returns an updated record value. Contained types keep their own
  mutability semantics.
- An array has one element type and integer indexing.
- A table preserves selected Luau collection behavior with statically known key
  and value types. An indexed read returns an optional value; indexed assignment
  inserts or replaces an entry without changing the invariant table type. Only
  key types with accepted canonical equality and hashing are indexable. See ADR
  0046.

Specialized ordered/persistent maps are ordinary nominal standard-library types
built on these primitives rather than a separate core language type.

Literal checking is context-sensitive. Without an expected type, the checker
uses documented rules and reports ambiguity rather than choosing an overly broad
type.

Changing a collection's shape cannot change its type. Heterogeneous content must
be expressed with a union/interface and checked on read.

## Classes and interfaces

Classes are nominal. A `ClassId` plus generic arguments determines identity.
Fields are selected by resolved `FieldId`; methods by `MethodId` and dispatch
category.

Interfaces expose a statically declared method/property contract. Implementation
is nominal: a class explicitly names each implemented interface, and an
interface value has a known interface type and dispatch table shape.

The initial surface contains instance methods only. A class `implements` clause
is checked against exact accessible receiver-method parameter/result/effect
signatures. Class-to-interface conversion is a static implicit upcast. Interface
calls retain the `InterfaceId` and resolved interface method; matching shape
without the clause is rejected.

Downcasts are explicit and return a typed optional/result. There is no universal
object type offering reflection or string member access.

## Functions, methods, and type packs

Function types include parameter types, result type pack, generic parameters,
and effects relevant to compile time, suspension, error propagation, unsafe
operations, and optimization.

Nested functions have the same fully typed function types. Capture analysis
records lexical `CaptureId`s. Read-only captures are by value; any captured
binding that is written uses one shared typed cell. Local assignment is checked
against the binding's fixed type and never changes its shape.

Luau-like multiple returns use a statically described type pack. A variadic tail
has a known repeated element type or a generic type-pack parameter with solver
constraints. It is never an untyped bag of values.

The first implemented pack form is fixed and exact. A parenthesized function
result annotation defines its element types, comma return syntax constructs it,
and multiple local declaration/assignment projects it by static index. A comma
list of scalar right-hand sides must also match the target arity exactly. There
is no Lua-style `nil` padding, extra-value truncation, or arbitrary last-value
expansion. See ADR 0045.

Indexing a tuple or fixed pack requires a positive in-range integer literal and
produces the exact element type at that position. A computed index is rejected;
tuple projection never introduces a common element type or dynamic lookup.

Colon method syntax affects receiver insertion at parsing/HIR construction, not
the underlying ability to type-check the call.

## Generics

The semantic checker treats generics parametrically and records constraints on
type parameters. Lowering is hybrid: value/performance-critical instantiations
specialize, while representation-compatible reference instantiations may share
typed dictionary/witness-based code. The choice cannot change accepted programs.

Generic instantiation:

1. creates fresh inference variables or accepts explicit arguments;
2. applies parameter bounds/constraints;
3. checks call arguments with expected instantiated parameter types;
4. solves and validates all remaining variables;
5. records canonical generic arguments in HIR.

The bootstrap subset currently requires explicit function and tagged-union case
type arguments, supports ordered invariant parameters on functions, records,
and tagged unions, and fully specializes every reachable concrete instance in
MIR. Equivalent instances are deduplicated. Type-argument inference, portable
generic reference metadata, and representation-compatible typed code sharing
remain later extensions of the same semantic model. See ADR 0050.

Compile-time generic code may branch on permitted type/attribute queries. Each
accepted branch still produces ordinary fully typed HIR.

## Subtyping and conversions

Subtyping is explicit per type family. Initial relationships include:

- `Never` below all inhabited types;
- singleton values below their primitive type;
- a type below a union containing it;
- class-to-base/interface relationships;
- variance declared and verified on read-only/write-only interfaces; mutable
  collections remain invariant;
- optional/union normalization.

Numeric widening, narrowing, signedness change, optional injection, interface
upcast, and checked downcast are distinct conversion kinds in HIR. No conversion
is labeled “dynamic.” Lossy conversions require explicit syntax unless an ADR
proves a safe implicit rule.

ADR 0040 fixes numeric conversion syntax as a call whose callee is a built-in
numeric type, for example `UInt32(value)`. The checker resolves that type in the
type namespace, requires one numeric operand, and records the exact source and
target kinds. It is never an ordinary overload or runtime lookup. Decimal
floating-point literals remain context-sensitive literal typing rather than an
implicit conversion from an existing value.

## Effects and compile-time eligibility

Function types or summaries carry effects needed to prove compile-time safety:

- allocation category;
- mutation scope;
- suspension;
- failure/unwind;
- unsafe memory;
- FFI;
- ambient I/O;
- permitted compiler-query capabilities.

The user-facing notation can remain minimal. The compiler may infer effect
summaries for ordinary functions, while explicit attributes state stronger
contracts such as required compile-time eligibility.

The bootstrap compiler uses no source effect punctuation. It infers the least
fixed point over typed local operations and resolved direct-call edges,
including recursive strongly connected components. Calls through function
values consume the closed summary stored in the function type; they never
substitute an unknown or all-effects fallback. Interface implementations cannot
widen the exact member summary they implement.

The canonical initial summary is closed and records allocation, managed
mutation, trap, panic/unwind, suspension, unsafe memory, FFI, ambient I/O, and
compiler-query capabilities. There is no unknown/dynamic effect. At each call,
the callee summary must be a subset of the caller summary.

## Attribute typing

An UDA constructor is type-checked like a constant constructor call. Attachment
adds target-kind and visibility checks. Attribute queries return a static option,
tuple/list, or predicate type based on repeatability; they never return an
untyped metadata value.

Symbol/type handles exist only in the compile-time type universe. Escape
analysis rejects storing them in runtime globals, fields, returned runtime
values, or retained metadata projections.

## Type checking output

For each accepted body, the checker publishes:

- the type of every expression/place;
- resolved locals, fields, methods, functions, namespaces, Modules, and Bubbles;
- explicit conversions;
- generic arguments and substitutions;
- dispatch categories;
- flow facts at relevant control points;
- effect summary;
- typed compile-time dependencies;
- diagnostics and source-origin chains.

HIR construction consumes this output without re-solving types.

## Soundness and validation strategy

- Unit tests cover normalization, substitution, unification, subtyping, and
  inference-variable scoping.
- Golden tests cover successful inferred types and negative diagnostics.
- Property tests generate types/constraints and check normalization idempotence,
  substitution laws, and solver determinism.
- HIR verification independently checks all recorded types and conversions.
- Differential backend tests prove no backend silently adds type-dependent
  behavior.
- Fuzzing must treat solver nontermination and exponential blowups as bugs;
  explicit complexity budgets produce diagnostics rather than fallback types.
