# ADR 0106: Bounded Foundation and Generated FFI Editor Analysis

- Status: accepted
- Date: 2026-07-26
- Supersedes: none
- Amends: ADR 0090, ADR 0093, ADR 0094

## Context

ADR 0090 deliberately limits coherent private editor analysis to
dependency-free Bubbles until a reusable Workspace and dependency query exists.
That limit must not make the private language server forget the reserved
`Pop.Standard` reference required by every normal Bubble. It also leaves
manifest-owned generated FFI Modules with misleading `unknown name`
diagnostics, even when an exact direct `Pop.Ffi` dependency and verified
ADR 0093/0094 sidecar already prove those names and callback contracts.

Running `pop check` and scraping its output would violate the structured tooling
contract. Treating `.popc` as ordinary Pop source would violate its bounded
typed-metadata role. Selecting the host target would also invent a target that
the open generated path may not own.

## Decision

The version-coupled private compiler tooling projection embeds and analyzes the
toolchain's exact `Pop.Standard` source once, publishes only its verified public
reference metadata, and supplies that reference to every normal editor
analysis. The embedded projection is compiler-version-coupled and uses reserved
tooling Bubble identities. It is not a Package cache, ambient namespace, or
replacement dependency.

ADR 0090's general dependency-analysis exclusion remains. One bounded exception
is accepted for an active generated FFI Module below the nearest Package:

1. the Package has an exact direct normal dependency whose selected Bubble is
   `Pop.Ffi`;
2. the dependency source is a local path naming a regular, non-symlinked
   `bubble.toml`;
3. that manifest has the exact `Pop.Ffi` Package identity and dependency
   version and conventionally discovers the `Pop.Ffi` library Bubble;
4. the active package-relative path exactly equals one configured generator's
   `outputDirectory/bindings.pop`;
5. exactly one platform generator section owns that path, so the path selects
   its declared target without consulting the host; and
6. the normal bounded ADR 0093/0094 preflight verifies the descriptor hash,
   generated inventory, source, and `native-bindings.popc` before callback
   attachments enter front-end analysis.

The `.popc` descriptor and sidecar remain typed generator inputs and are never
parsed as ordinary `.pop` Modules. A failed preflight publishes its intrinsic
stable `POP5080` through `POP5087` diagnostic on the active generated Module and
suppresses dependent unknown-name or missing-callback cascades. It never
silently drops the failure or converts it to final rendered text inside a
compiler pass.

Registry, exact-Git, platform dependency, sibling-Bubble, arbitrary extension,
and complete Workspace graph resolution remain outside this decision. No FFI
name becomes ambient: without the verified exact direct dependency, ordinary
static unknown-name diagnostics remain.

## Consequences

- Standard-library names resolve consistently in CLI and private editor
  analysis through the same compiler-verified public contract.
- Exact generated callback declarations receive their verified sidecar facts
  and no longer produce false `Ffi.*` or end-of-file diagnostics.
- Invalid or stale generated output reports the generator's stable root-cause
  diagnostic instead of a secondary callback-contract failure.
- The bounded local-path exception duplicates a small amount of manifest
  verification until the reusable Workspace query supersedes it.
- The language server still cannot claim general cross-Bubble analysis.

## Alternatives considered

### Make `Ffi` and Standard names ambient editor builtins

Rejected because `Pop.Standard` is a verified implicit Bubble reference and
`Pop.Ffi` is an explicit dependency, not a universal global namespace.

### Parse `.popc` files as source Modules

Rejected because descriptors and generated sidecars are bounded typed metadata,
not source, and may contain generator-only declarations.

### Select the host target

Rejected because editor analysis may inspect cross-target output and tools may
not fabricate an undeclared target choice.

### Implement general dependency analysis in the same slice

Rejected because registry locks, exact-Git resolution, artifact loading,
platform selection, and complete Workspace invalidation require a separate
accepted design and broader conformance.

## Required conformance tests

- standalone and dependency-free same-Bubble analysis both receive the verified
  reserved `Pop.Standard` reference;
- an exact local direct `Pop.Ffi` dependency plus canonical generated callback
  source and sidecar produces no FFI, unknown-name, or recovery diagnostics;
- the same FFI names without that dependency remain unknown;
- malformed, stale, missing, extra, or hash-mismatched selected output publishes
  the exact `POP5080` through `POP5087` root cause without dependent
  unknown-name or missing-callback cascades;
- a path owned by zero or multiple platform generator sections attaches no
  sidecar and never falls back to the host target; and
- `.popc` contents never enter ordinary source discovery or parsing.

## Documents/components affected

Private language-server analysis, compiler tooling projections, Standard
reference bootstrapping, generated FFI preflight, diagnostic adaptation,
architecture conformance tests, closed decisions, and the implementation
roadmap.
