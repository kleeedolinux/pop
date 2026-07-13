# ADR 0042: Numeric For Ranges and Loop Control

- Status: accepted
- Date: 2026-07-13
- Supersedes: the first-slice loop-control deferral in ADR 0032

## Context

Pop Lang has typed `while` and `repeat` loops, while the accepted Luau feature
inventory also requires `for`, `break`, and `continue`. Generalized iteration
depends on the nominal `Iterable<T>` and `Iterator<T>` protocols and must not be
approximated with dynamic calls, string member lookup, or universal tables.
Integer range iteration can be specified independently and provides the first
closed, statically typed `for` form.

## Decision

Pop Lang adds the Luau-shaped numeric range statement:

```luau
for index = first, last, step do
    visit(index)
end
```

`first`, `last`, and the optional `step` are evaluated exactly once from left to
right before the loop. All three values have one identical fixed integer type.
An omitted `step` is the value `1` of that type. The range includes `last` when
the progression reaches it: a positive step continues while `index <= last`,
and a negative step continues while `index >= last`. A zero step raises the
typed `InvalidRangeStep` trap before the first iteration; a statically known
zero step is rejected. Progression uses
the integer type's checked arithmetic and traps on overflow.

The loop binding is a new immutable local visible only in the body. It may be
captured by a closure but cannot be assigned. The bounds and step cannot be
observed through implicit globals or runtime reflection.

`break` exits the innermost enclosing `while`, `repeat`, or numeric `for`.
`continue` advances the innermost enclosing loop:

- in `while`, it proceeds to the condition;
- in `repeat`, it proceeds to the `until` condition;
- in numeric `for`, it proceeds to the checked range advancement.

Both are statements without values or labels and are rejected outside a loop.
A `repeat` condition may continue to read body locals, but a `continue` must not
bypass initialization of such a local; the initial implementation
conservatively rejects a `repeat` that combines a body-local declaration with a
`continue` targeting that loop.

HIR preserves numeric range iteration and loop-control statements as typed
source concepts. Canonical MIR lowers them to ordinary integer operations,
conditional branches, block arguments, traps, and backedges. It adds no range,
iterator, `break`, or `continue` opcode. Backedge safe-point requirements remain
unchanged and apply equally to explicit `continue` edges.

Generalized `for binding in iterable do` remains deferred until the nominal
iteration protocol, specialization, disposal, and multiple-binding behavior
are accepted together. This decision does not make a range a runtime object or
add a range operator; `..` remains string concatenation.

## Consequences

- Pop Lang gains deterministic integer ranges and structured loop exits without
  weakening static typing.
- Numeric `for` retains familiar Luau punctuation and `do`/`end` blocks.
- LLVM and the MIR interpreter consume identical verified CFG semantics. The
  runtime-free C subset continues to reject loops whose required safe points
  exceed its accepted capability.
- `Iterable<T>` and `Iterator<T>` remain nominal, distinct library/compiler
  protocols rather than dynamic fallback hooks.

## Alternatives considered

### Use `start..finish` as a range expression

Rejected because Luau and ADR 0041 reserve `..` for string concatenation. An
overloaded range meaning would make parsing and type-directed reading less
predictable.

### Add generalized iteration immediately

Rejected because the callable protocol, iterator lifetime, multiple-binding,
and specialization contracts are not yet closed. Inventing them in lowering
would violate the architecture-first rule.

### Make loop control backend instructions

Rejected because structured control flow is completely represented by MIR CFG
edges and block arguments.

## Required conformance tests

- parser tests for explicit/default step forms, nesting, and malformed clauses;
- exact same-integer-kind typing, immutable binding, zero-step rejection, and
  rejection of loop control outside loops;
- inclusive ascending and descending execution, empty ranges, evaluation order,
  `break`, and each loop-specific `continue` target;
- nested-loop tests proving innermost targeting and closure-boundary rejection;
- HIR preservation and MIR verification, textual round trips, block arguments,
  checked progression, and safe points on every backedge;
- MIR interpreter/LLVM differential behavior and deterministic C capability
  rejection where the required runtime contract is unavailable;
- negative scans proving there is no dynamic iterator lookup, range opcode,
  implicit global, or overloaded numeric `..` behavior.

## Documents/components affected

Language model, intermediate representations, implementation roadmap, closed
decisions, Luau inventory, syntax and nomenclature, conformance matrix, syntax,
type checking, HIR, MIR lowering and verification, and backend differential
tests.
