# ADR 0063: First Standard API Baseline Expansion

- Status: accepted
- Date: 2026-07-14
- Depends on: ADRs 0058, 0061, and 0062
- Supersedes: none

## Context

ADR 0058 fixes the initial `Pop.Standard` prelude and a canonical append-only
API baseline. ADRs 0061 and 0062 authorize the first additional ordinary Pop
implementations for `Sequence` and `Math`, but a source declaration alone must
not bypass the baseline that distinguishes executable prototypes from stable
compatibility promises.

The prelude is already accepted and includes foundational identities that are
needed by later language and runtime work. Extending ordinary APIs does not
justify removing, renumbering, or changing those identities.

## Decision

The version-1 baseline keeps every ADR 0058 row unchanged and appends:

- one non-prelude `Pop.Math` namespace row;
- the `Sequence` APIs authorized by ADRs 0061, 0064, 0066, and 0067; and
- the `Math` APIs authorized by ADRs 0062 and 0065.

Every added row has `standard` tier, `prototype` status, and `prelude = false`.
The functions are callable and cross-backend tested, but prototype status does
not promise source compatibility for the first stable release. Advancing a row
to `implemented` still requires the complete ADR 0058 documentation, compiled
example, cost, artifact, interpreter, and LLVM evidence gate.

`Sequence` remains the only implicit namespace root. `Math` is reached through
`Math.name` when a surrounding context already resolves `Math`, or through an
explicit `using Pop.Math`. No new low-priority namespace candidate is added.

API identities are append-only and ordered by baseline number. They are
compatibility identities for the public snapshot, not compiler-known function
IDs, HIR/MIR operations, native symbols, or bootstrap intrinsics.

The exact authoritative list remains
`libraries/standard/bootstrap/api-baseline.tsv`. Architecture tests compare the
declared public Pop functions with that snapshot so a declaration cannot become
public without a row and a row cannot claim a missing implementation.

## Consequences

- The accepted prelude and every existing stable identity remain unchanged.
- Public source growth is reviewable independently of compiler bootstrap data.
- Implemented behavior and stable compatibility remain distinct.
- `Math` does not widen the global name-resolution surface.
- Later additions must append identities rather than reorder this first
  expansion.

## Alternatives considered

### Replace the ADR 0058 prelude

Rejected because the merge established it as the current authority. Removing
task, cancellation, comparison, hashing, cleanup, or identifier identities in
an unrelated algorithm PR would break architecture traceability and upcoming
runtime work.

### Derive rows automatically from source

Rejected because source discovery proves implementation, not tier, prelude,
status, compatibility identity, or documentation ownership.

### Mark every new API implemented immediately

Rejected because execution tests alone do not satisfy the full ADR 0058
stability gate. Prototype is an honest executable state.

### Add `Math` to the prelude

Rejected because the call-site gain does not justify another implicit root in
this slice. Collision and compatibility review remain separate.

## Required conformance tests

- every ADR 0058 row and prelude binding remains byte-for-byte compatible;
- added rows use unique ascending identities and exact static signatures;
- the baseline and ordinary public Pop declarations agree in both directions;
- only `Sequence` is an implicit namespace root;
- planned catalog roots do not enter the executable baseline; and
- malformed, reordered, duplicate, unknown-status, and bootstrap-disagreement
  inputs remain rejected.

## Documents/components affected

The API baseline, standard-library baseline loader tests, architecture
conformance tests, public catalog, closed decisions, examples, contributor
guidance, and implementation roadmap.
