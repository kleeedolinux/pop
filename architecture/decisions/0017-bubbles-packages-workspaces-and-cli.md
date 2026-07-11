# ADR 0017: Bubbles, Packages, Workspaces, and Unified CLI

- Status: accepted
- Date: 2026-07-10
- Amends: ADR 0007

## Context

The earlier architecture used “library” for compilation, reference, artifact,
and loading concerns while leaving Package/Workspace/CLI behavior underspecified.
That ambiguity is poor for tooling and monorepos. Rust/Cargo demonstrates a
strong separation among items/modules, crates, packages, and workspaces, plus a
cohesive command workflow.

## Decision

Pop Lang adopts the hierarchy `Item → Module → Bubble → Package → Workspace`.
`Bubble` is the Pop name for the crate-equivalent independently compiled unit.
A Module is one `.pop` file, a Package is a directory with `[package]` in
`bubble.toml`, and a Workspace is a group of Packages sharing `bubble.lock`,
resolution, output, and policy.

Conventional roots are `src/lib.pop`, `src/main.pop`, `src/bin/`, `tests/`,
`examples/`, and `benchmarks/`. One Package can contain multiple Bubbles.
`internal` visibility stops at the Bubble boundary; Package/Workspace membership
does not widen it.

The unified command is `pop`, with Cargo-like build/check/run/test/doc,
dependency, metadata, packaging, and Workspace selection workflows. Dependency
requirements live in `bubble.toml`; exact resolution lives in `bubble.lock`.

During compiler bootstrap, `pop check <source.pop> --dump hir|mir` also provides
a standalone Module inspection mode. The driver creates an ephemeral
Workspace, Package, and one-Module Bubble for that invocation so the normal
ownership hierarchy remains intact. Those session-local identities do not come
from the filename and are not Package discovery, dependency resolution,
artifact identity, or a compatibility promise. Standalone inspection cannot
declare Bubble dependencies, exercise cross-Module visibility, emit build
artifacts, or populate Package/Workspace caches. ADR 0024 separately authorizes
a native bootstrap `pop build <source.pop> --output <executable>` and `pop run
<source.pop>` path for one-Module conformance examples. That path emits an
explicitly requested disposable executable; it does not create a Package
artifact, infer publishable identity, or populate Package/Workspace caches.

`--dump hir` and `--dump mir` are repeatable debugging controls on `pop check`.
HIR is printed only after successful HIR verification, and MIR only after
successful canonical MIR verification. The dumps are deterministic for one
compiler version but are not stable serialization formats. Diagnostics go to
standard error, and a failed check prints no partial IR to standard output.

Reusable library Bubbles continue to emit `.poplib` artifacts. Source semantics
use `BubbleIdentity`; “library” describes a Bubble/artifact kind or the standard
library, not a competing ownership level.

## Consequences

- Compiler graphs, HIR/build metadata, diagnostics, and caches carry Bubble IDs.
- Package resolution produces the Bubble dependency graph consumed by the
  compiler and linker.
- Monorepos share one lockfile/output cache without becoming one compilation or
  visibility boundary.
- The CLI and language server share structured metadata/diagnostic protocols.
- Early front-end and MIR work has an inspectable vertical slice without
  pretending that an arbitrary source path has publishable Package/Bubble
  identity.
- Earlier library-identity and library-scoped `internal` wording migrates to
  `BubbleIdentity` and same-Bubble visibility.
- Namespace organization remains independent of filesystem/package ownership.

## Alternatives considered

### Call the unit a crate

Rejected because Pop Lang should adopt the proven model without importing Rust
terminology as language identity.

### Make Package the compilation unit

Rejected because a Package commonly needs a reusable library plus multiple
binaries/tests/tools with distinct dependency and entry-point graphs.

### Let Workspace membership widen `internal`

Rejected because monorepo layout would silently change encapsulation and public
architecture.

### Use separate compiler, package, formatter, and documentation commands

Rejected as the primary UX because one `pop` command can provide consistent
selection, diagnostics, configuration, and machine protocols.

## Required conformance tests

- conventional and explicit Bubble target discovery;
- non-overlapping Module ownership and same-Bubble `internal` visibility;
- Package/Workspace selection and virtual Workspace behavior;
- deterministic `bubble.lock`, offline/locked/frozen modes, and resolver tests;
- local/registry/Git dependency identity and hash verification;
- CLI human/JSON parity and deterministic multi-Package diagnostics;
- standalone Module HIR/MIR dump determinism, verification-before-output,
  invalid-source/invalid-option rejection, and no partial dump on failure;
- bootstrap standalone native build/run rejects ambiguous entries and emits an
  executable only at the explicit output path;
- monorepo incremental-cache isolation and reuse;
- package archive reproducibility and forbidden build-script capability tests.

## Documents/components affected

Language/project nomenclature, resolver, compiler driver, HIR/build metadata,
artifact loading, manifests, base libraries, diagnostics, language server,
formatter, test/doc runners, package manager, registry protocol, cache, and CI.
