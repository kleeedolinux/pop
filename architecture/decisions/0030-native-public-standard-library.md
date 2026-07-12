# ADR 0030: Native Public Standard-Library Architecture

- Status: accepted
- Date: 2026-07-12
- Supersedes in part: ADR 0009 public BCL direction; ADR 0012 public catalog
- Refined by: ADR 0031 public roots and ADR 0032 concise/cost contracts

## Context

`Pop.Standard` was described as BCL-inspired. Although prior documents rejected
copying .NET APIs, that framing still made a foreign object-oriented library the
reference architecture. It also mixed a small bootstrapped public surface with
promises for frameworks, platform services, and optional ecosystems.

## Decision

The public library is designed for Pop, with mature-platform breadth used only
as a capability checklist. Its API shape is functions, immutable records and
unions, typed protocols, opaque resource handles, explicit capability values,
and generated typed adapters. Classes are permitted only for real identity or
stateful protocol resources; they are never the organizing default.

The authoritative catalog is architecture section 22 and its numbered domain
documents. They classify every planned root by distribution tier, dependencies,
portability, security, extension contract, cost, implementation phase, and
status. No planned entry claims implementation.

Public APIs use typed `Result` errors, explicit resource lifecycle, closed
effects, cancellation/deadline values, and capability requirements. They cannot
depend on broad reflection, string dispatch, dynamic maps, ambient global
services, inheritance trees, or runtime registration by name.

## Rationale

Pop Lang's static, Luau-shaped, data/function-first model needs a library whose
abstractions preserve those properties. Broad capability coverage is valuable,
but a foreign compatibility surface would turn names and object patterns into
an accidental language design. Tiers let stable portable contracts grow without
making every program carry platform, server, UI, media, or AI dependencies.

## Consequences

- `Pop.Standard` is a portable foundation, not an indivisible framework.
- Networking, databases, UI, media, scientific computing, and AI are official
  package families unless their small portable value contract belongs in core.
- Public contracts are backend-neutral; native/runtime bindings are behind PLRI
  and typed capability checks.
- Existing bootstrap APIs remain implementation evidence only and are audited
  against the catalog as they grow.

## Compatibility impact

The current implemented public surface is a bootstrap, not a mature released
API. Planned catalog names do not imply source compatibility. Existing public
names remain only when they pass the ADR 0031 naming and API-shape review;
otherwise a staged deprecation, compatibility adapter, or deliberate breaking
change is recorded before release.

## Security impact

Every implemented I/O, process, parser, archive, network, cryptographic, and
credential API must define safe defaults, typed limits, trust boundaries, and
explicit unsafe/capability access. The architecture forbids ambient authority
and runtime extension discovery because both obscure reviewable trust paths.

## Portability impact

Portable contracts cannot depend on one backend or operating system. Target
adapters live below the portable tier and report typed unsupported capability
outcomes where a compile-time target requirement does not apply. PLRI remains
the only backend/runtime service boundary.

## Implementation impact

Implementation proceeds by catalog phase with API metadata, documentation,
negative security tests, and shared backend conformance before stabilization.
This ADR does not authorize a bulk implementation or empty package skeletons.

## Alternatives considered

### Continue adapting the .NET BCL

Rejected because its historical object model and compatibility surface distort
Pop's data/function-first design even when individual names are changed.

### Ship one monolithic public Bubble

Rejected because it forces unrelated dependencies, obscures portability and
security boundaries, and prevents disciplined release cadence.

### Make every capability third-party

Rejected because text, collections, formats, I/O, time, errors, and typed
resource/concurrency contracts need one dependable portable baseline.

## Migration

Section 22 owns the inventory and migration sequence. Architecture documents
move first; bootstrap APIs are then classified, retained, renamed, deprecated,
or moved only through focused feature ADRs and conformance work.

## Unresolved questions

- Exact language syntax for error propagation, scoped cleanup, and async.
- The initial fixed prelude snapshot and foundation Package publication model.
- Which portable HTTP, crypto, and database value contracts are practical before
  the first supported runtime targets exist.

## Required conformance tests

- tier dependency graph and public-reference metadata snapshots;
- API-shape checks rejecting forbidden service/factory/manager patterns;
- capability, unsupported-operation, resource, cancellation, and parser-limit
  conformance tests for each implemented domain;
- cross-backend tests for every portable public contract.

## Documents/components affected

Base libraries, library loading, naming, roadmap, diagnostics, metadata,
runtime/PLRI boundaries, public API baselines, documentation, and architecture
conformance policy.
