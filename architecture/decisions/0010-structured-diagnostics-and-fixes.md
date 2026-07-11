# ADR 0010: Structured Diagnostics, Warning Waves, and Quick Fixes

- Status: accepted
- Date: 2026-07-10

## Context

Diagnostics must serve terminals, editors, CI, compile-time execution, and
incremental compilation. Plain message strings are unstable, difficult to test,
and insufficient for reliable automatic fixes. New warnings can also break
warning-as-error builds without a compatibility policy.

## Decision

Built-in diagnostics use stable `POP####` codes, typed message arguments,
primary/related spans, origin chains, intrinsic severity/category, optional
warning wave, and structured quick fixes.

New compatibility-sensitive warnings enter numbered opt-in waves. Projects can
configure groups/IDs, promote warnings to errors, and narrowly suppress warnings
with a required reason. Errors and compiler incidents cannot be suppressed.

Quick fixes consume semantic facts and produce versioned atomic workspace edits.
They declare `Safe`, `RequiresReview`, or `Unsafe` applicability. Only composing
safe fixes support unattended fix-all. Dependency downloads/manifest additions
require a separate previewed action.

Human, JSON, LSP, SARIF, and test output render from the same diagnostic model.

## Consequences

- A machine-readable catalog and generated typed constructors are required.
- Compiler passes retain reason/origin data useful for diagnostics and fixes.
- Warning policy does not contaminate intrinsic semantic query results.
- IDE and CLI output remain consistent.
- Fix providers need versioning, conflict detection, postcondition checks, and
  deterministic tests.

## Alternatives considered

### Format strings directly in compiler passes

Rejected because message text would become an accidental API and fixes would
lack reliable semantic inputs.

### Enable every new warning immediately

Rejected because warning-as-error users need a deliberate upgrade path.

### Auto-apply all suggested edits

Rejected because many valid corrections involve user intent. Applicability and
preview are part of the fix contract.

