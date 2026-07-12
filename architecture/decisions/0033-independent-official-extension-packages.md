# ADR 0033: Independent Official Extension Packages

- Status: accepted
- Date: 2026-07-12
- Supersedes in part: ADR 0031 root placement and section 22 Package ownership

## Context

The first public-library catalog classified AI, RPC, data, command, and syntax
tooling domains as optional, but it did not establish installable repository
artifacts for them. Language tooling also needs a stable public LSP contract
that is distinct from the compiler's private Rust language-server and syntax
implementation crates.

Keeping these domains in the catalog without independent manifests, versions,
dependencies, and builds would make “official Package” only a documentation
label. Moving their APIs into the automatically referenced foundation would
force unrelated applications to carry tooling, database, AI, or protocol
surfaces.

## Decision

The repository contains six official, independently versioned extension
Packages:

| Package | Owned public namespaces | Direct official dependencies |
| --- | --- | --- |
| `Pop.Data` | `Pop.Data`, `Pop.Sql`, `Pop.Store` | none |
| `Pop.Ai` | `Pop.Ai` | `Pop.Data` |
| `Pop.Cli` | `Pop.Cli`, `Pop.Command`, `Pop.Settings` | none |
| `Pop.Rpc` | `Pop.Rpc` | none |
| `Pop.Syntax` | `Pop.Syntax`, `Pop.Source` | none |
| `Pop.Lsp` | `Pop.Lsp` | `Pop.Rpc`, `Pop.Syntax` |

Every Package has its own `bubble.toml`, library Bubble, version, dependency
list, source root, public documentation boundary, and independent build target.
They may share a repository and release automation, but one Package cannot rely
on another Package's unpublished source tree or `internal` declarations.

All six use normal Package resolution. They are not prelude members, implicit
references, components of `Pop.Standard`, or automatically installed with the
toolchain/application. The request term `Pop.Std` is treated as referring to the
standard foundation; the canonical reserved identity remains `Pop.Standard`
under ADR 0009 and the complete-name rule.

The initial repository implementation is a bootstrap: it establishes verified
manifests, dependency direction, namespace ownership, and independently
buildable host metadata crates. It does not claim that the planned domain APIs
are implemented or that registry installation already exists.

## Domain boundaries

`Pop.Data` owns typed datasets, schemas, rows/columns, SQL contracts, and
non-SQL storage. It does not absorb common formats: `Json`, `Yaml`, `Xml`,
`Csv`, `Toml`, and `Codec` remain portable standard contracts.

`Pop.Ai` owns vendor-neutral model, inference, training, generation, embedding,
search, evaluation, and runtime-adapter contracts. Dataset/search integration
uses the explicit `Pop.Data` dependency. Vendor/runtime adapters remain
separately installable members of the `Pop.Ai.*` family.

`Pop.Cli` owns command models, argument parsing, help/completion, and typed
settings composition. Low-level standard streams and terminal capability facts
remain in portable `Terminal`; full-screen terminal UI remains a future optional
Package rather than becoming implicit through `Pop.Cli`.

`Pop.Rpc` owns typed request/response/streaming schemas and generated stubs.
It uses standard `Codec`, `Json`, `Task`, `Http`, and telemetry contracts without
requiring `Pop.Data`. Protocol adapters remain `Pop.Rpc.*` Packages.

`Pop.Syntax` is a stable public facade for source text, tokens, immutable
lossless syntax trees, typed edits, and versioned syntax schemas. It does not
re-export compiler arenas, recovery internals, query databases, resolved AST,
HIR/MIR, or the private Rust `pop-syntax` crate API mechanically.

`Pop.Lsp` owns versioned LSP/JSON-RPC protocol values, capabilities, messages,
workspace edits, semantic tokens, diagnostics transport, and test transports.
It depends on the public `Pop.Syntax` facade and typed `Pop.Rpc` contracts. The
official language server will implement its public protocol boundary with
`Pop.Lsp`, but its compiler/query engine remains private tooling.

## Rationale

These Package boundaries match independent reasons to install and version the
domains. They keep ordinary applications small while allowing official tooling
and ecosystems to share stable typed contracts. Public facades prevent existing
private compiler crates from accidentally becoming compatibility surfaces.

## Alternatives considered

### Put all extensions in `Pop.Standard`

Rejected because it makes every application depend on unrelated APIs and couples
their release cadence to the toolchain foundation.

### Publish one `Pop.Extensions` Package

Rejected because it recreates a monolith, hides dependency direction, and
prevents independent versioning and installation.

### Re-export current Rust compiler/tool APIs

Rejected because their ownership, IDs, recovery nodes, and incremental-query
contracts are private implementation details rather than reviewed Pop APIs.

### Make `Pop.Lsp` depend directly on compiler crates

Rejected because the protocol/data contract must remain usable by editors,
proxies, tests, and alternative tooling implementations without embedding the
compiler.

## Compatibility impact

These Packages are new and planned APIs have no source-compatibility promise.
The previous catalog roots are reassigned rather than aliased. `Command` and
`Settings` move under `Pop.Cli`; `Sql` and `Store` move under `Pop.Data`; `Source`
moves under `Pop.Syntax`. Automated migration is deferred until concrete public
signatures exist.

## Security and portability impact

Extensions inherit the standard typed-error, capability, limit, and no-runtime-
reflection rules. Each manifest declares target/native requirements. Syntax and
LSP inputs are untrusted and bounded; RPC limits messages and recursion; CLI
redacts secrets; data drivers parameterize queries; AI credentials/models remain
explicit. Portable extension contracts cannot depend on backend-specific HIR or
MIR behavior.

## Implementation impact

The repository workspace gains six host bootstrap crates and six Pop Package
roots. Architecture tests validate exact identities, namespace ownership,
versions, manifests, dependency edges, non-membership in the prelude/standard
bootstrap, and independent Cargo builds. Registry install/publish behavior and
full Pop-source compilation remain later package-tooling work.

## Required conformance tests

- exact Package/Bubble/namespace inventory and unique ownership;
- manifest parsing and conventional library-Bubble discovery;
- dependency graph checks, including no cycles or undeclared edges;
- absence from `Pop.Standard` bootstrap/prelude metadata;
- independent build for every host bootstrap crate;
- no dependency from public extension metadata to compiler-private Rust crates;
- later, API/cost/security/cross-backend tests per implemented domain.

## Migration

Catalog and roadmap ownership change first. Initial Packages expose only
bootstrap identity/status metadata. Existing compiler syntax and language-server
crates remain private until focused PRs design and implement public facade types.

## Unresolved questions

- Registry coordinates, signing policy, and install/publish CLI implementation.
- Whether extension versions share a coordinated compatibility train in
  addition to independent semantic versions.
- The first stable public API slice for each Package.

## Documents/components affected

Public standard-library index/catalogs, Package/loading architecture, tooling
architecture, implementation roadmap, Cargo workspace, extension manifests and
sources, architecture tests, and future package-manager commands.
