# ADR 0036: Typed Cross-Bubble Function References

- Status: accepted
- Date: 2026-07-12
- Depends on: ADR 0001, ADR 0003, ADR 0007, ADR 0017, and ADR 0035
- Supersedes: none

## Context

ADR 0035 establishes conventionally discovered Pop source Modules for the two
base-library Bubbles. A public function can now be analyzed and lowered inside
`Pop.Standard`, but a dependent Bubble cannot yet resolve or type-check a call
to it. The compiler currently treats `SymbolId` as compilation-local even
though architecture defines it as stable only inside one Bubble.

Giving contributed functions new compiler-known bootstrap IDs would preserve
the old high-friction workflow and make ordinary library APIs compiler policy.
Combining dependency source with consumer source would collapse Bubble identity,
visibility, initialization, and artifact boundaries.

## Decision

A cross-Bubble symbol is identified by the typed pair
`SymbolIdentity { bubble: BubbleId, symbol: SymbolId }`. Bare `SymbolId` remains
valid only with an already-known owning Bubble. HIR and MIR retain the complete
identity for referenced direct calls; they never recover an owner from a source
name, ABI spelling, or runtime lookup.

The first reference-metadata implementation publishes public namespace
functions with:

- their `SymbolIdentity`, namespace, source name, and public visibility;
- ordered parameter names and fully typed parameter/result descriptions;
- the closed effect summary required to type and verify calls;
- the owning Bubble dependency identity.

`internal` and `private` declarations and function bodies are excluded. A
consumer loads metadata only for direct resolved Bubble dependencies. Resolution
uses the normal current-namespace, `using`, alias, and qualified-name rules and
then binds one exact `SymbolIdentity`. Metadata never creates a dependency from
a `using` directive.

The bootstrap slice supports canonical primitive types in public function
signatures. A public signature containing a type not yet representable in the
versioned metadata schema prevents reference emission with a toolchain error;
it never becomes an unknown type, string-dispatched call, erased ABI value, or
dynamic fallback. Records, unions, generics, interfaces, and portable generic
bodies extend the same closed typed schema in focused later slices.

Logical metadata emission/loading and cross-Bubble HIR/MIR conformance land
before the deterministic on-disk encoding and complete `.poplib` directory.
The later encoding is a serialization of this verified model, not a second
semantic path. Until implementation objects are linked or supplied to the MIR
interpreter, tests may prove resolution, typing, identity, visibility, and IR
retention without claiming execution.

Local compiler arenas may remap dependency entries to session-local IDs for
storage efficiency, but every published HIR/MIR call retains the original
`SymbolIdentity`. Remapping cannot change public identity or leak into artifact
metadata.

## Consequences

- A contributor can add an ordinary primitive-signature function to a
  `Pop.Standard` Module without editing bootstrap function tables or compiler
  lowering registries.
- Consumers statically resolve and type-check that function through public
  metadata while preserving Bubble visibility boundaries.
- HIR/MIR and future backend symbol selection have an unambiguous portable
  cross-Bubble identity.
- The initial metadata type vocabulary is deliberately small; unsupported
  public signatures fail closed until their typed schema is implemented.
- Disk artifact encoding, implementation linking, richer public types, and
  cross-backend execution remain explicit follow-up work.

## Alternatives considered

### Add every standard function to the compiler bootstrap table

Rejected because ordinary APIs would continue to require coordinated resolver,
HIR, MIR, interpreter, and backend edits.

### Use a globally unique bare `SymbolId`

Rejected because the accepted identity contract scopes `SymbolId` to a Bubble,
and global allocation would make independent artifact compilation unstable.

### Load dependency source into the consumer Bubble

Rejected because it collapses visibility, ownership, initialization, and
artifact boundaries and makes source availability a dependency requirement.

### Resolve metadata calls by names at runtime

Rejected because Pop Lang has no unchecked member/function lookup, runtime
reflection registry, or dynamic call fallback.

## Required conformance tests

- metadata emits public functions and omits `internal`/`private` declarations;
- duplicate/malformed identities and unsupported signature types fail closed;
- a direct dependency resolves qualified and `using`-shortened public calls;
- non-dependencies and inaccessible declarations remain unresolved;
- wrong arity and argument types fail during static checking;
- HIR and MIR retain the exact `SymbolIdentity` without ABI/source-name lookup;
- local symbol-number collisions across Bubbles do not change identity;
- adding a contributed primitive-signature function requires no bootstrap,
  compiler protocol, HIR operation, MIR operation, or backend registry edit.

## Documents/components affected

Foundation IDs, reference metadata, resolver, type checker, HIR, MIR, base
libraries, test runner, `.poplib` implementation plan, and architecture
conformance policy.
