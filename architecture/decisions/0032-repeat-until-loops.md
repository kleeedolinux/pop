# ADR 0032: Luau-Shaped Repeat-Until Loops

- Status: accepted
- Date: 2026-07-12
- Supersedes: none

## Context

Pop Lang adopts Luau-shaped control flow, but its first executable subset only
implemented pre-condition `while` loops. Programs also need a concise loop that
executes its body before testing a statically checked exit condition.

## Decision

Pop Lang adds the following statement:

```luau
repeat
    value = value + 1
until value == 3
```

The body executes at least once. The `until` expression is evaluated after
each body execution and must have type `Boolean`; `true` exits the loop and
`false` starts the next iteration. `repeat` has no `do` or final `end` because
`until` closes the construct.

One lexical scope covers both the body and the condition. A local declared in
the body is available to the corresponding `until` condition but is not visible
after the loop. The first slice adds neither `break` nor `continue`.

HIR preserves the body and condition as a distinct typed statement. Canonical
MIR lowers it to ordinary body, condition, exit, and backedge CFG blocks; it
does not add a backend-specific opcode or runtime operation. Existing loop
backedge safe-point verification applies.

## Consequences

- Pop Lang gains a familiar body-first loop without JavaScript/C-style syntax.
- The condition cannot use truthiness, dynamic values, or implicit conversion.
- LLVM and the MIR interpreter execute the same verified MIR control flow. The
  runtime-free experimental C backend diagnoses the required backedge safe
  point as an unsupported runtime operation under ADR 0031; it must not insert
  a no-op PLRI fallback.
- Generalized `for` iteration, `break`, and `continue` remain separate designs.

## Alternatives considered

### Encode body-first loops with `while true`

Rejected because it obscures the exit condition and makes a common Luau-shaped
control-flow form unnecessarily verbose.

### Add a backend-specific repeat instruction

Rejected because canonical MIR already represents structured loops as portable
control-flow blocks and edges.

## Required conformance tests

- parser acceptance, nesting, and missing-`until` diagnostics;
- exact Boolean condition typing and no dynamic/truthiness fallback;
- body-local visibility in the condition and non-visibility after the loop;
- at-least-once execution and mutation across the backedge;
- HIR/MIR verification, deterministic dumps, and safe-point insertion;
- MIR interpreter and LLVM behavioral equivalence, plus deterministic C-backend
  capability rejection without partial output.

## Documents/components affected

Language model, syntax and nomenclature, Luau relationship inventory,
implementation roadmap, syntax parser, type checker, compile-time checker,
HIR, MIR, and cross-backend conformance tests.
