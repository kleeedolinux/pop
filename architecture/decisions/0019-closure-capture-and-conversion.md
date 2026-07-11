# ADR 0019: Lexical Closures and Explicit Capture Conversion

- Status: accepted
- Date: 2026-07-10
- Supersedes: none

## Context

Pop Lang promises Luau-like first-class functions and closures, while the
accepted IR architecture requires captured state to become explicit before a
backend sees it. The existing architecture did not fix the source forms,
capture ordering, or mutation rule precisely enough for independent backends to
agree.

## Decision

Pop Lang accepts Luau-shaped local functions and anonymous function
expressions. Parameters and results of a nested function follow the same typed
signature rules as namespace functions; inference never creates an untyped
parameter or result.

A nested function resolves names lexically. A referenced local or parameter
owned by an enclosing function is a capture. Capture analysis assigns stable
typed capture identities and orders an environment by lexical declaration
identity, never by textual name or hash iteration.

Read-only captures are copied into an immutable environment field. A binding
written by either its declaring scope or any closure is represented by one
shared typed capture cell, so all closures observe source-order mutation. Local
assignment remains statically typed. A `local function` binding is visible in
its own body and uses a capture cell when recursion or mutual capture requires
one. Shadowed bindings remain distinct identities.

HIR retains nested functions, capture identities, capture mode, and typed
closure construction. Closure conversion produces backend-neutral MIR code
functions plus explicit environment allocation, loads, stores, and indirect
calls. A non-capturing function may lower to a plain typed code reference.
Closure environments are native managed objects, not tables, metatables, or
runtime name maps.

## Consequences

- Captured mutation has one portable meaning across the interpreter and native
  backends.
- Escaping closures keep their environment alive through ordinary precise GC.
- Capture conversion and its verifier become mandatory before backend handoff.
- Implementations may elide environments/cells only when identity and mutation
  observations are preserved.

## Alternatives considered

### Capture every binding through a mutable box

Rejected because read-only captures need no shared mutation identity and the
extra allocation would obscure the language's data-flow semantics.

### Capture values from a runtime name table

Rejected as a Lua regression and a violation of strong static typing.

### Defer capture mode to each backend

Rejected because captured mutation and closure identity are language semantics.

## Required conformance tests

- non-capturing, read-only capturing, mutating, recursive, escaping, and nested
  closures;
- shadowed bindings and deterministic capture ordering;
- rejection of assignment with the wrong static type;
- verifier rejection of missing, duplicate, mistyped, or wrongly owned captures;
- interpreter/optimized-MIR differential tests, including collection at every
  closure-allocation safe point;
- permanent tests proving environments are not tables and calls never use
  runtime string lookup.

## Documents/components affected

Language model, syntax and nomenclature, type system, HIR, MIR, runtime/GC,
reference interpreter, driver, and backend conformance tests.
