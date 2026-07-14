# ADR 0056: First-Class Integer Ranges

- Status: accepted
- Date: 2026-07-14
- Supersedes: the no-runtime-range restriction in ADR 0042 and the undefined
  `Range<TInteger>` construction contract in ADR 0053

## Context

ADR 0042 defines the inclusive Luau-shaped numeric `for` clause and explicitly
avoids a first-class range value. ADR 0053 later requires `Range<TInteger>` to
implement the reserved nominal iteration protocol, but does not define how a
program constructs that value, which integer types it accepts, or how its
iterator preserves the numeric-range traps. Implementing the required
collection without closing those public contracts would make compiler behavior
the architecture by accident.

## Decision

`Pop.Standard` supplies the reserved immutable `Range<TInteger>` value. Its
type argument must be one exact fixed integer type, including the accepted
`Int` and `Byte` aliases. Floating-point, decimal, user-defined, union, optional,
and dynamically selected numeric types are rejected.

The first public constructor is the type-companion function:

```luau
local ascending = Range.create(1, 5)
local descending = Range.create(5, 1, -2)
```

Its conceptual signature is:

```luau
public function create<TInteger>(
    first: TInteger,
    last: TInteger,
    step: TInteger = 1,
): Range<TInteger>
```

`TInteger` is inferred from `first`; `last` and an explicit `step` must have the
same canonical integer type. The omitted step is the exact value `1` of that
type. Arguments evaluate exactly once from left to right. A statically known
zero step is rejected and a dynamic zero step raises `InvalidRangeStep` before
an iterator can yield an item.

The progression is the one from ADR 0042: it includes `last` when reached,
positive steps continue through `<=`, negative steps continue through `>=`,
and advancement uses checked arithmetic. Empty direction-mismatched ranges
yield no items. Overflow raises the existing checked-arithmetic trap only when
advancement is required to determine a later item; completing at an already
yielded inclusive endpoint does not perform an unnecessary overflowing add.

`Range<TInteger>` implements exactly `Iterable<TInteger>`. Each `iterator()`
call returns one independent single-pass `Iterator<TInteger>` session. A
generalized `for` therefore retains ADR 0053's once-only source evaluation,
once-only acquisition, one `next()` call per step, `break`/`continue`, lexical
cleanup, and no-implicit-disposal rules. A range is immutable, so structural
mutation and concurrent modification do not apply.

HIR retains a typed `RangeCreate` expression with the exact integer type and
three evaluated operands. MIR retains one backend-neutral typed `RangeCreate`
operation and otherwise uses the ordinary reserved iteration-interface calls,
discriminant tests, branches, checked arithmetic, traps, and safe points fixed
by ADRs 0042 and 0053. No backend reconstructs a range from syntax, and no
string lookup, dynamic iterator call, range opcode for loop control, or
overloaded `..` operator is introduced.

The bootstrap native implementation may allocate one small immutable managed
range value and one iterator-session object. These allocations are part of the
checked cost contract. A backend may scalar-replace either object when
identity, rooting, safe points, traps, and observable evaluation order remain
unchanged. The versioned native ABI adds a typed range-construction adapter and
the closed range iteration kind; its payload is always the exact statically
known integer representation.

## Consequences

- The generalized-iteration collection inventory is constructible and can be
  tested without adding a range operator.
- Numeric `for` remains the shortest loop syntax; `Range.create` is the value
  form used when a range must be passed, stored, or consumed generically.
- `Range` stays distinct from arrays, lists, tables, iterator sessions, and
  namespaces.
- Bootstrap allocations are documented rather than hidden, while portable
  optimization remains possible.

## Alternatives considered

### Treat the numeric `for` clause as a `Range` expression

Rejected because the clause is a statement form, its comma-separated syntax is
not an expression, and ADR 0042 already gives it direct typed HIR semantics.

### Add a `first..last` expression

Rejected because `..` is the accepted `String` concatenation operator and
overloading it would drift from Luau's readable surface.

### Lower generalized range iteration directly to a numeric loop

Rejected because it would bypass the exact `Iterable<T>` acquisition contract
and make range behavior a special lowering exception rather than a reserved
nominal protocol implementation.

### Represent a range as an array or list

Rejected because it would materialize every item, change allocation and
overflow behavior, and collapse distinct semantic collection concepts.

## Required conformance tests

- bootstrap identity, exact arity, prelude status, and integer-only generic
  validation;
- two/three-argument construction, same-kind inference, once-only left-to-right
  evaluation, omitted-step typing, and malformed-call diagnostics;
- ascending, descending, stepped, singleton, empty, inclusive-endpoint,
  zero-step, and checked-overflow behavior;
- independent iterators, once-only generalized-loop acquisition, loop control,
  nesting, lexical cleanup, and explicit absence of implicit disposal;
- HIR/MIR construction and verifier negatives, textual MIR round trips,
  optimizer preservation, safe points, and root maps;
- MIR-interpreter/LLVM differential execution and explicit experimental-C
  rejection; and
- regression scans excluding dynamic calls, Lua iterator triples, overloaded
  numeric `..`, array/list materialization, and backend-specific HIR/MIR.

## Documents/components affected

Language model, syntax/nomenclature, closed decisions, standard-library
catalog and documentation, bootstrap metadata, type checking, HIR, MIR,
runtime interface/native ABI, interpreter, LLVM, experimental-C capability
validation, conformance matrices, and roadmap.
