# ADR 0029: Default Internal Visibility and Empty Results

- Status: accepted
- Date: 2026-07-11
- Supersedes in part: ADR 0015 explicit-modifier requirement

## Context

ADR 0015 made every declaration that supports visibility spell a modifier.
That made Bubble-local implementation declarations noisier than their intended
default while retaining little review value: `public` and `private` are the
meaningful boundary choices, and `internal` is the ordinary Bubble-local case.

The function grammar already represents an omitted result annotation as an
empty result pack, but the architecture did not state that rule generally.
The language must preserve fully explicit parameter and value-return types
without treating an omitted result as an inference request.

## Decision

When a declaration form accepts `public`, `internal`, or `private`, omitting
the modifier assigns `internal`. This applies to namespace-scope functions,
constants, type aliases, attributes, records, unions, classes, interfaces, and
enums, and to class fields and methods. Explicit `public` and `private` retain
their existing semantics. Interface members remain public by their containing
interface contract and do not gain visibility syntax.

The binary-root `function main(...)` shorthand remains the sole exception.
When its visibility is omitted, the target-aware entry contract assigns
`private`, preserving ADR 0026's entry-point visibility semantics. Explicit
`public` and `internal` remain invalid binary-entry visibility choices. A
library `main` is an ordinary namespace function and therefore defaults to
`internal`.

An omitted function return annotation denotes an explicit empty result pack.
It is the no-value (`void`) form: `return` may carry no values, fallthrough is
valid, and a valued `return` is rejected. An annotation is required for every
non-empty result pack. The compiler never infers a return type, including for
basic types, and parameters remain explicitly typed.

The declaration-index and typed semantic representations always carry the
resolved visibility and result pack. HIR, MIR, and backends therefore receive
no optional visibility or inferred/dynamic result state.

## Consequences

- Ordinary Bubble-local declarations use concise Luau-shaped syntax.
- Public and file-private boundaries remain explicit and reviewable.
- The binary entry point preserves its private target-boundary contract.
- Omitting a return annotation cannot accidentally change a function's API;
  adding a returned value requires an explicit result annotation.
- The obsolete missing-visibility diagnostic and its quick fix are removed.

## Alternatives considered

### Keep explicit visibility everywhere

Rejected because `internal` is the common Bubble-local case and the repeated
modifier adds ceremony without making a boundary decision visible.

### Default omitted visibility to private or public

Rejected because private would make ordinary cross-Module implementation
sharing unexpectedly fail, while public would silently expand a Bubble's
reference metadata surface.

### Infer basic return types

Rejected because it makes function APIs depend on body analysis, complicates
separate declaration checking and recursive functions, and weakens the rule
that non-empty function results are explicit static contracts.

## Required conformance tests

- parser tests cover omitted and explicit visibility for every namespace
  declaration kind and class field/method;
- declaration-index tests prove omitted ordinary visibility resolves to
  `internal` while omitted `main` resolves to `private`;
- resolver and metadata tests prove default-internal declarations stay inside
  the declaring Bubble and outside public reference metadata;
- syntax and type tests prove omitted result annotations accept only empty
  returns and reject valued returns without a return-type inference path;
- interface-member tests prove their public-by-contract syntax remains
  unchanged.

## Documents/components affected

Syntax, declaration indexing, resolution, typed declaration parsers, body
checking, diagnostics, HIR construction inputs, conformance tests, the
language model, syntax specification, entry-point contract, and architecture
regression policy.
