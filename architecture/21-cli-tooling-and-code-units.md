# CLI, Tooling, and Units of Code

## Canonical hierarchy

Pop Lang uses one precise hierarchy:

```text
Workspace
└── Package
    └── Bubble
        └── Module
            └── Item
```

The names are not interchangeable.

| Unit | Meaning | Identity boundary |
| --- | --- | --- |
| Item | A declared function, type, constant, attribute, field, or case | stable `SymbolId` inside a Bubble |
| Module | One `.pop` source file | private visibility and initialization ownership |
| Bubble | One independently compiled target, analogous to a Rust crate | `internal` visibility, dependency, metadata, code generation, and linking |
| Package | One publishable/versioned directory whose `bubble.toml` contains `[package]` | dependency-version and distribution identity |
| Workspace | A group of packages developed and resolved together | one lock graph, output tree, policy, and command-selection root |

A namespace is orthogonal to this ownership hierarchy. It organizes item names
across modules in one Bubble but is not a file, Bubble, package, runtime object,
or dependency unit.

## Items

An item is a namespace-scope declaration or a declared member/case owned by one.
Namespace-scope items explicitly use `public`, `internal`, or `private`.

- `public` crosses a Bubble boundary through reference metadata;
- `internal` crosses Module boundaries only inside the same Bubble;
- `private` stops at the declaring Module;
- a Package or Workspace never widens visibility automatically.

Two Bubbles in the same Package are still dependency peers. One cannot access
the other's `internal` items merely because they share `bubble.toml`.

## Modules

One `.pop` source file is one Module. It owns source spans, private items, XML
documentation attachment, incremental invalidation, and any explicit module
initialization. A Module declares exactly one file-scoped namespace.

Modules do not return values and are not imported with `require`. They are not
tables. All module-to-module references resolve statically through namespaces
inside the Bubble or public APIs of dependency Bubbles.

Module identity is the owning `BubbleId` plus its normalized path relative to
the Bubble source root. Moving a Module can change its private/module identity
but does not change the identity of unchanged public items whose stable
definition keys are preserved by tooling.

## Bubbles

A Bubble is the smallest independently compiled Pop Lang program unit and is
the direct equivalent of a Rust crate in the project model. Each Bubble has:

- one name and kind;
- one root Module;
- a deterministic set of owned Modules;
- a direct Bubble dependency list;
- one language edition and target-capability contract;
- one public reference-metadata surface;
- one initialization graph;
- one `HirBubble` aggregate and one or more MIR/code-generation units;
- one output artifact family.

Initial Bubble kinds are:

- `library`: reusable public API, emitting reference metadata and a `.poplib`;
- `binary`: executable entry point;
- `test`: isolated test executable;
- `example`: executable example;
- `benchmark`: benchmark executable.

“Target” in CLI/manifest documentation means a Bubble selected for building.
“Platform target” means an architecture/OS/backend triple. Diagnostics and
machine schemas always use the unambiguous terms `bubble` and `platformTarget`.

A Bubble is not a namespace. It may contain many namespaces, and a namespace
cannot span Bubbles. A library Bubble's compiled `.poplib` is an artifact of the
Bubble, not a second source-level unit called a library.

## Packages and `bubble.toml`

A Package is a directory whose `bubble.toml` contains `[package]`. It supplies package identity,
version, dependencies, Bubble targets, publishing metadata, and tool policy.
One Package may contain one library Bubble and any number of binary, test,
example, and benchmark Bubbles.

Minimal manifest:

```toml
[package]
name = "Studio.Gameplay"
version = "0.1.0"
edition = "2026"

[dependencies]
StudioData = "2.1"
```

Manifest keys use `camelCase`; package and Bubble identities use `PascalCase`
components. Dependency keys are local aliases and follow the identity casing.
The manifest format is versioned independently from the language edition.

The Package version applies to every published Bubble in the Package. A
`BubbleIdentity` consists of the exact package identity/version/source plus the
Bubble name, public API hash, and relevant ABI/capability facts. Bubbles do not
invent unrelated versions inside one Package.

## Conventional Package layout

The zero-configuration layout is:

```text
gameplay/
├── bubble.toml
├── src/
│   ├── lib.pop
│   ├── main.pop
│   ├── players.pop
│   └── bin/
│       ├── migrate.pop
│       └── server/
│           ├── main.pop
│           └── routes.pop
├── tests/
│   └── saveRoundTrip.pop
├── examples/
│   └── basicServer.pop
├── benchmarks/
│   └── decoding.pop
└── resources/
```

Auto-discovery rules:

- `src/lib.pop` is the default library Bubble root;
- `src/main.pop` is the default binary Bubble root;
- each `src/bin/*.pop` is an additional single-Module binary Bubble;
- each `src/bin/<name>/main.pop` roots a multi-Module binary Bubble owning that
  directory;
- each `tests/*.pop`, `examples/*.pop`, and `benchmarks/*.pop` roots its matching
  Bubble kind;
- ordinary `.pop` files under `src/`, excluding reserved target roots/directories,
  belong to the library Bubble;
- when no `src/lib.pop` exists, the default `src/main.pop` Bubble owns ordinary
  `.pop` Modules under `src/` instead;
- binary/test/example/benchmark Bubbles depend on the Package's library Bubble
  through its public API when `src/lib.pop` exists.

The default library and `src/main.pop` Bubble names equal the Package name.
Additional target filenames/directories use `camelCase` and derive a
`PascalCase` Bubble name (`assetCompiler.pop` → `AssetCompiler`). A collision or
noncanonical derivation is an error with a manifest-override quick fix; the tool
never performs Cargo-style dash/underscore identity rewriting.

A binary root must resolve exactly one entry item. The minimal form is:

```luau
function main()
    print(42)
end
```

The explicit full form remains
`private function main(arguments: Array<String>): Int`. Entry visibility may be
omitted or explicitly `private`; an omitted binary-root entry remains private
even though ordinary omitted visibility defaults to internal. Parameters may be
absent or exactly `arguments: Array<String>`; the result may be absent or exactly `Int`. Normal
completion of a no-result entry means status zero. `arguments` excludes the
executable path. Every requested argument must be valid UTF-8 and is preserved
exactly, including empty and non-ASCII strings. Invalid platform argument bytes
cause a closed runtime trap before an argument-taking `main` executes rather
than lossy conversion.
Applications keep typed `Result` errors internally and translate them explicitly
at the entry boundary. Async programs call the typed `Async.run` adapter rather
than adding hidden async entry behavior. Entry selection uses `SymbolId` during
compilation, never runtime string lookup.

Library Bubbles do not resolve an entry item. A Package may build `src/lib.pop`
and `src/main.pop` together without imposing the binary's `main` contract on the
library.

A Package without `src/lib.pop` is valid. A Bubble can be declared explicitly
when the conventional layout is insufficient:

```toml
[bubble.library]
name = "Studio.Gameplay"
root = "source/gameplay.pop"

[[bubble.binary]]
name = "GameplayServer"
root = "tools/server/main.pop"
```

Explicit ownership must be non-overlapping. The same Module cannot be compiled
into two Bubbles accidentally. Shared code belongs in a library Bubble and is
used through a declared dependency.

## Workspaces and monorepos

A Workspace groups Packages under a root `bubble.toml`. The root may also be a
Package, or it may be a virtual Workspace containing only `[workspace]`.

```text
studio/
├── bubble.toml
├── bubble.lock
├── packages/
│   ├── gameplay/
│   │   ├── bubble.toml
│   │   └── src/lib.pop
│   └── data/
│       ├── bubble.toml
│       └── src/lib.pop
├── tools/
│   └── assetCompiler/
│       ├── bubble.toml
│       └── src/main.pop
└── target/
```

```toml
[workspace]
members = ["packages/*", "tools/*"]
defaultMembers = ["packages/gameplay"]
resolver = "1"

[workspace.package]
edition = "2026"
license = "MIT"

[workspace.dependencies]
StudioData = { path = "packages/data", version = "2.1" }

[workspace.diagnostics]
warningWave = 3
warningsAsErrors = ["Correctness"]
```

Workspace rules:

- the nearest ancestor `bubble.toml` with `[workspace]` is the Workspace root;
- `members` and `exclude` are evaluated deterministically and may use restricted
  path globs;
- every member remains an independently publishable Package;
- one `bubble.lock` at the Workspace root locks all selected Package sources,
  versions, content hashes, features, and Bubble edges;
- one shared `target/` tree enables cross-Package incremental reuse;
- workspace package/dependency/diagnostic/profile values are inherited only when
  a member explicitly writes `workspace = true` for that field/dependency;
- commands select the current Package by default, or `defaultMembers` when run
  at a virtual Workspace root;
- `--workspace`, `--package <name>`, and `--bubble <name>` make selection explicit;
- duplicate Package identities or overlapping member roots are errors.

This supports large monorepos without turning the Workspace into one giant
visibility or compilation boundary.

## Dependencies and resolution

The Package manifest declares dependency requirements; the compiler consumes a
resolved Bubble graph.

```toml
[dependencies]
StudioData = { version = "2.1", bubble = "Studio.Data" }
HttpCodec = { git = "https://example.invalid/http-codec", revision = "8f31..." }

[developmentDependencies]
TestSupport = { workspace = true }

[platform."x86_64-linux".dependencies]
NativeTls = "1.4"
```

Initial dependency sources are registry, exact Git revision, and normalized
local path. A dependency can rename/select a public library Bubble, but a path
never becomes semantic identity. Normal dependencies are available to selected
library/binary Bubbles; development dependencies are limited to tests,
examples, and benchmarks.

### Features

Package features are named additive manifest capabilities. Initially they may
enable optional dependencies, resources, or explicitly gated Bubble targets.
They cannot inject source, change parsing, create declarations, or become
runtime-dynamic flags. Conditional source compilation requires a separate
accepted design; it is not smuggled in through Cargo compatibility.

The selected feature set is stored in `bubble.lock`, included in
`BubbleIdentity` and cache keys, and exposed by `pop metadata`. Public API
baselines are feature-set-specific. Resolver unification is deterministic for
one Package identity within a Workspace.

Resolution rules:

1. read the Workspace and selected Package manifests;
2. resolve Package versions/sources once for the Workspace;
3. select requested public library Bubbles from those Packages;
4. validate editions, platform targets, PLRI ABI, and capabilities;
5. write/verify `bubble.lock` deterministically;
6. construct the acyclic compile-time Bubble graph and separately validate the
   runtime initialization graph;
7. load only public reference metadata during compilation;
8. select implementation artifacts only for linking/execution.

`--locked` rejects lockfile changes. `--offline` forbids network access.
`--frozen` requires both. Builds never modify a lockfile implicitly when one of
those modes is active. Credentials never appear in `bubble.toml`, `bubble.lock`,
diagnostics, or cache keys.

## Unified `pop` CLI

`pop` is the single user-facing command, analogous in role to Cargo plus the
formatter/linter entry points. Compiler internals may be separate processes,
but users and editors consume one stable command and machine protocol.

Core commands:

| Command | Contract |
| --- | --- |
| `pop new` / `pop initialize` | Create a Package or Workspace using canonical layout |
| `pop check` | Resolve and type-check through HIR/MIR verification without final native linking |
| `pop build` | Build selected Bubbles and dependencies |
| `pop transpile` | Experimentally emit a selected backend source artifact without compiling it |
| `pop run` | Build and run exactly one binary/example Bubble |
| `pop test` | Build and run unit, integration, and XML documentation tests |
| `pop benchmark` | Build/run benchmark Bubbles under an explicit profile |
| `pop documentation` | Check XML docs and emit documentation for public library Bubbles |
| `pop format` | Check or apply the canonical formatter |
| `pop lint` | Run warning/analyzer policy without changing source |
| `pop fix` | Apply structured safe fixes; review fixes require confirmation |
| `pop add` / `pop remove` | Edit dependencies transactionally and update resolution |
| `pop update` | Deliberately update selected locked dependencies |
| `pop tree` | Explain the Package and Bubble dependency graph |
| `pop metadata` | Emit the resolved Workspace/Package/Bubble graph as versioned JSON |
| `pop package` / `pop publish` | Verify and create/publish a deterministic Package archive |
| `pop install` | Build/install a selected public binary Bubble |
| `pop clean` | Remove selected build outputs, never source/manifests/lockfiles |

Shared selectors and controls include:

```text
--manifestPath <path>
--workspace
--package <PackageName>
--bubble <BubbleName>
--library
--binary <BubbleName>
--example <BubbleName>
--test <BubbleName>
--platformTarget <triple>
--profile <name>
--locked
--offline
--frozen
--messageFormat human|json
```

Long option spelling follows manifest/source nomenclature and avoids arbitrary
abbreviations. Short flags are limited to established interactive conveniences
and never become the only documented interface.

### Standalone compiler inspection

During compiler bootstrap, one Module can be checked without a manifest through
the explicit debug-oriented form:

```text
pop check path/to/example.pop --dump hir
pop check path/to/example.pop --dump mir
pop check path/to/example.pop --dump hir --dump mir
```

The driver supplies an ephemeral Workspace, Package, Bubble, Module, and
namespace compilation context for that invocation. The source path is an input,
not semantic identity. This mode does not perform Package/Bubble discovery,
resolve dependencies, widen visibility, emit build artifacts, or populate
normal Package/Workspace caches. It therefore cannot replace manifest-driven
selection for a real program.

`--dump hir` and `--dump mir` are repeatable and preserve request order. The
driver completes front-end diagnostics, verified HIR construction, and
canonical MIR verification before writing any requested dump. Diagnostics are
written to standard error; failure writes no partial IR to standard output.
Dump text is deterministic for a compiler version and is a test/debug format,
not a stable serialization or machine compatibility contract.

### Experimental C transpilation

The accepted bootstrap form is:

```text
pop transpile path/to/example.pop --to c
```

Like standalone inspection, this direct path supplies an ephemeral
Workspace/Package/Bubble/Module context and resolves no dependencies. It
requires a canonical binary entry; the runtime-free first slice accepts only a
no-argument entry with no result or an `Int` result. The driver completes source
analysis, verified MIR construction, portable optimization, and C-backend
validation before writing the complete deterministic C11 translation unit to
standard output. Diagnostics go to standard error, and failure emits no partial
C source.

`--to` names a deliberate backend source format, not a platform target. Only
`c` is accepted initially. The C artifact and its private helpers are
experimental and have no stable spelling or ABI promise. Stable typed integer
and literal-string output use private C standard-I/O adapters without the Pop
runtime. Unsupported managed allocation, PLRI, dispatch, unwind, coroutine,
unsafe, or FFI operations are capability errors rather than unchecked C
fallbacks. See
[ADR 0031](./decisions/0031-experimental-secure-c-transpilation-backend.md).

`lib.pop` and `src/bin/` are reserved conventional filesystem names requested
by the Package layout. They do not authorize `Lib`, `Bin`, or other truncated
identifiers in Pop source/public APIs.

`pop run` fails with a selection diagnostic when more than one runnable Bubble
matches. `pop check --workspace` may operate in parallel but emits diagnostics
in deterministic Package/Bubble/Module/span order.

## Test units

In-source unit-test items belong to the tested Bubble. They can exercise
`internal` items, and same-Module tests can access `private` items. Test harness
metadata never enters the public API.

Each `tests/` root is a separate integration-test Bubble and can use only the
public API of the Package library Bubble plus declared development dependencies.
Examples and benchmarks are separate Bubbles under the same rule.

## Tooling architecture

The CLI, language server, editor extensions, documentation generator, formatter,
test runner, and build servers use the same compiler/query APIs. They do not
shell out to scrape human text.

Stable machine-facing contracts are:

- structured diagnostics and workspace edits;
- `pop metadata --messageFormat json` with an explicit schema version;
- newline-delimited build events for progress/artifacts/diagnostics;
- deterministic HIR/MIR/debug dumps for the compiler version;
- dep-info files listing tracked source, manifest, environment capability, and
  generated inputs;
- cancellation and bounded editor queries;
- semantic symbol IDs for rename/navigation/XML `cref`.

Human output is not parsed as an API. Tools must pass an explicit Workspace,
Package, Bubble, Module, and platform target selection rather than guessing from
artifact filenames.

## Profiles, cache, and reproducibility

Initial profiles are `Development`, `Release`, `Test`, and `Benchmark`.
Workspace roots may define profiles; member Packages opt into inherited values.
Profile selection changes optimization/debug/assertion policy, never language
semantics or public API.

The default output/cache root is `<workspace>/target/` (or `<package>/target/`
outside a Workspace). It contains profile/platform/Bubble outputs, dependency
artifacts, incremental state, generated documentation, and test executables.
Its internal layout is explicitly unstable and never used as Package identity.

Cache keys include compiler version, language edition, Bubble graph, normalized
source and manifests, locked dependency identities, target capabilities, PLRI
ABI, profile, permitted environment inputs, and compile-time dependencies.
Absolute checkout paths and timestamps do not affect reproducible artifacts.

## Publishing and supply-chain rules

`pop package` creates a deterministic archive containing the Package manifest,
selected source/resources, license/readme metadata, and a file-hash inventory.
It excludes `target/`, credentials, editor state, and undeclared files.

`pop publish` requires a clean package verification build, complete public XML
documentation, API/naming baselines, declared licenses, and registry policy.
Registry protocol is replaceable and not part of language semantics. Signed
Package metadata binds package identity/version/source hashes; compiled Bubble
artifacts bind their `BubbleIdentity` and public API hash.

Build scripts are not an ambient shell escape. If a future build-tool Bubble is
accepted, it runs as a separately declared capability-limited tool dependency
with tracked inputs/outputs and cannot inject source strings into the compiler.

## Cargo influence boundary

Pop Lang adopts Cargo's successful separation of packages, crate-like build
targets, conventional roots, workspaces, lockfiles, selection flags, and a
unified workflow. It does not copy Rust syntax, procedural macros, unrestricted
build scripts, crate visibility rules, underscore-normalized identities, or the
name `crate`. The Pop unit is **Bubble**, and Pop visibility/namespace/static
typing rules remain authoritative.

Primary structural references:

- [Cargo targets](https://doc.rust-lang.org/cargo/reference/cargo-targets.html)
- [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html)
- [Cargo dependency specification](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html)
- [Cargo build commands](https://doc.rust-lang.org/stable/cargo/commands/build-commands.html)
