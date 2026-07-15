# ADR 0073: Implicit Standard Artifact Linking

- Status: accepted
- Date: 2026-07-14
- Depends on: ADRs 0035, 0036, 0055, and 0058
- Supersedes: the source-only dependency and raw-object linking staging in the
  public library implementation plan

## Context

Every normal Pop Lang Bubble is architecturally entitled to one trusted,
toolchain-compatible `Pop.Standard` reference. The compiler already embeds the
exact prelude identities and can emit verified `.poplib` artifacts, but the CLI
does not yet compile and inject the ordinary `Pop.Standard` source surface.
Tests compensate by adding `sequence.pop` and `math.pop` manually. The package
linker also links the raw object used to create an artifact instead of reading
back the target implementation selected from the verified artifact.

This makes executable prototypes unavailable through the ordinary daily CLI
workflow and leaves artifact verification beside, rather than on, the path to
the final executable.

## Decision

The toolchain reserves Bubble IDs 1 and 2 inside one compilation session for
`Pop.Internal` and `Pop.Standard`. User and dependency Bubbles are allocated
from 3 upward. These numeric IDs are session-local typed identities; Package,
Bubble, source, and public-API hashes remain the serialized identity contract.

The CLI discovers every Module below the repository-owned
`crates/libraries/standard/pop` Package through the ordinary Package convention,
analyzes the reserved `Pop.Standard` Bubble against the reserved Internal
identity, and injects its public reference metadata as a direct dependency of
every normal source or Package Bubble. This selection is a toolchain trust root,
not a user dependency, runtime registry, environment lookup, or namespace
search. The exact prelude remains the ADR 0058 snapshot.

Package builds emit `Pop.Standard` and ordinary library Bubbles as `.poplib`
artifacts. After emission, the build reloads and verifies each artifact, selects
the implementation whose target exactly matches the requested target, writes
that verified byte sequence to the linker input, and never links the unverified
pre-emission object. A target mismatch or missing implementation fails closed.

Direct source `check`, `build`, and `run` receive the same implicit Standard
reference. Native direct-source builds emit the portable Standard object beside
the program object before linking. The trusted Rust `pop-standard` archive
remains only the reviewed native-adapter implementation for bootstrap functions
such as `print`; it is not the source of portable Sequence or Math declarations.

The first implementation continues compiling local-path dependency sources
before artifact emission. Fully source-free cache/registry consumption requires
deterministic remapping of session-local Bubble IDs from multiple independently
built artifacts. Until that mapping is accepted and implemented, the driver
must not guess identity from filenames, namespaces, dependency aliases, or raw
numeric IDs.

## Consequences

- `Sequence` and `Math` prototypes work through ordinary CLI commands without
  manual source inclusion.
- Adding a portable Standard Module requires no Rust or compiler registry edit.
- Verified `.poplib` implementation bytes become the actual package linker
  inputs.
- The fixed trust root and prelude remain deterministic and non-extensible by
  user Packages.
- Registry/cache consumption remains an explicit follow-up rather than an
  unsafe partial loader.

## Alternatives considered

### Embed every Standard API in compiler tables

Rejected because portable declarations and bodies belong to ordinary Pop
Modules. Compiler tables remain limited to trusted bootstrap roles.

### Discover a Package named Pop.Standard on the dependency path

Rejected because an ordinary Package must not replace the toolchain trust root
or inject prelude names.

### Link the object that was used to create the artifact

Rejected because it leaves artifact target selection and integrity verification
off the executable path.

### Reuse cached artifacts by numeric Bubble ID

Rejected because IDs are session-local. Independent artifacts may contain the
same raw ID for different Bubbles.

## Required conformance tests

- ordinary CLI source and Package checks resolve `Sequence` and `Math` without
  adding their source files to the application;
- a nearer declaration still shadows the prelude namespace root or function;
- user Bubbles cannot replace the reserved Standard identity;
- an added Standard `.pop` Module is conventionally discovered without a
  central registry edit;
- package linking reads the selected implementation back from a verified
  `.poplib` and rejects missing or mismatched target implementations;
- Standard reference metadata and portable specializations work through MIR
  interpretation and LLVM;
- direct-source and manifest builds produce the same observable results; and
- no runtime registration, filename identity, namespace dispatch, or dynamic
  fallback is introduced.

## Documents/components affected

Base-library architecture, public-library implementation plan, driver Package
lowering, `.poplib` selection, CLI integration tests, architecture tests, and
the roadmap.
