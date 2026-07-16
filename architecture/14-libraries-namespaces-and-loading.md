# Bubbles, Namespaces, Artifacts, and Loading

## Model

Pop Lang combines two useful separations:

- C#/.NET-like namespaces, `using`, reference metadata, and load contexts;
- Cargo-like Bubble targets, Packages, Workspaces, manifests, and lock graphs.

The canonical ownership hierarchy is defined in
[CLI, tooling, and units of code](./21-cli-tooling-and-code-units.md):

```text
Item → Module → Bubble → Package → Workspace
```

A **Bubble** is the compiler/reference/linking boundary analogous to a Rust
crate. A reusable library is a Bubble kind and `.poplib` artifact, not another
ownership layer between Bubble and Package.

## Namespaces

Every Module declares exactly one file-scoped namespace:

```luau
namespace Studio.Gameplay.Players
```

Namespace components use `PascalCase`. A namespace can span Modules inside one
Bubble, but cannot span Bubbles. Directory layout should normally mirror the
namespace for navigation; semantic identity comes from the declaration.

Namespaces contain explicitly visible items. They are compile-time symbol
scopes, not runtime objects or tables:

```luau
namespace Image

public function resize(image: ImageData, width: Int, height: Int): ImageData
end

internal function validateDimensions(width: Int, height: Int): Result<(), ImageError>
end

private const MAX_DIMENSION = 32768
```

`Image.resize` is resolved statically. No instance, utility class, module table,
or runtime member-name lookup exists.

## Visibility boundaries

- `public`: accessible to dependent Bubbles and emitted in reference metadata;
- `internal`: accessible across Modules in the same Bubble only;
- `private`: accessible only in the declaring Module/file;
- `local`: block/function-local and not namespace visibility.

Package and Workspace membership never widen visibility. Two Bubbles in the
same Package interact through declared dependencies and public APIs.

## Using directives

```luau
using Studio.Shared
using Physics = Studio.Simulation.Physics
```

A `using` makes accessible namespace names available without full qualification.
It:

- is valid only in the file header;
- creates no dependency, runtime initialization, or load operation;
- cannot import inaccessible `internal`/`private` items;
- cannot change or forward visibility;
- cannot be computed;
- reports unused, duplicate, ambiguous, and missing-Bubble diagnostics.

Package `bubble.toml` dependencies make Bubbles available; `using` only changes
static name lookup after that graph is resolved. HIR retains stable symbol IDs,
not an open namespace search.

`Pop.Standard` is the sole normal implicit Bubble reference. Its fixed trusted
`@Prelude` surface supplies root types/functions and selected namespace names.

Official extensions are ordinary explicit Package dependencies. ADR 0033 starts
with `Pop.Data`, `Pop.Ai`, `Pop.Cli`, `Pop.Rpc`, `Pop.Syntax`, and `Pop.Lsp`.
They have independent manifests, versions, dependency graphs, and library
Bubbles even when developed in this repository. No toolchain installation or
application receives them through the implicit `Pop.Standard` reference; normal
manifest resolution is always required.

## Modules, Bubbles, Packages, and Workspaces

| Term | Meaning |
| --- | --- |
| Module | One `.pop` file; private scope and initialization owner |
| Bubble | Independent compilation/reference/link target; `internal` boundary |
| Package | Versioned/publishable directory with `[package]` in `bubble.toml` |
| Workspace | Group of Packages sharing `bubble.lock`, resolver, cache, and policy |
| Namespace | Cross-Module name organization inside one Bubble |
| Application | Selected binary Bubble plus its closed dependency graph |
| Load context | Runtime Bubble identity cache and implementation resolver |

The toolchain supplies two reserved library Bubbles: `Pop.Internal` and
`Pop.Standard`. Their layering remains defined in
[Base libraries](./16-base-libraries.md).

## Package references and locked resolution

`bubble.toml` declares Package requirements and selects public library Bubbles:

```toml
[dependencies]
StudioData = { version = "2.1", bubble = "Studio.Data" }
StudioNetworking = { version = "4.0", bubble = "Studio.Networking" }
```

The Workspace resolver creates `bubble.lock` containing exact Package versions,
sources, revisions, content hashes, selected features/capabilities, and Bubble
edges. Source code never reaches into dependency directories. A local path is a
resolution source, not identity.

Compilation consumes the resolved Bubble graph. `using` directives can name
only namespaces available from the current Bubble or public reference metadata
of a direct dependency Bubble.

## Bubble artifacts

A reusable library Bubble emits a deterministic `.poplib` artifact:

```text
Studio.Gameplay.poplib/
  bubble.manifest
  reference.metadata
  documentation.xml
  targets/
    x86_64-linux/native.object
    aarch64-linux/native.object
    portable/popvm.bytecode
  resources/
```

The directory form may later be packed without changing the logical format.
Binary/test/example/benchmark Bubbles emit their corresponding executable and
debug/test metadata rather than pretending to be libraries.

ADR 0055 fixes the version-1 physical control files as bounded canonical UTF-8
JSON with identity-sorted arrays and exactly one trailing newline. Every
inventoried file has a recorded size and lowercase hexadecimal SHA-256 digest.
Paths are normalized relative paths and cannot traverse or escape the artifact.
`documentation.xml` and opaque target implementation files retain their native
formats. Emission verifies a complete temporary artifact through the normal
loader before atomic publication.

### Bubble identity

A `BubbleIdentity` contains:

- exact Package identity, version, and resolved source;
- Bubble name and kind;
- optional publisher/signing identity;
- public API and implementation content hashes;
- Pop Lang edition and manifest schema;
- PLRI ABI range;
- platform target and capability constraints.

Package/Bubble display names are not security identity. Hash and signature
verification occurs before executable content is mapped.

### Bubble manifest

`bubble.manifest` records:

- `BubbleIdentity` and supported platform targets;
- artifact files/resources and hashes;
- direct Bubble references with version/ABI constraints;
- public namespace index;
- Module initialization entry points and ordering edges;
- runtime/target capabilities and native foreign-library dependencies;
- canonical native link requirements, provider/version facts, local-input
  hashes, and ABI fingerprints from ADR 0081;
- optional signing information;
- whether this is a reference-only artifact;
- documentation hash/schema when present.

### Reference metadata

`reference.metadata` contains only what a dependent Bubble needs:

- public namespace and symbol names;
- public signatures and ABI-visible layouts;
- generic constraints and portable bodies required for specialization;
- interface/virtual contracts;
- public compile-time-relevant UDA values;
- public constants;
- referenced `BubbleIdentity` values.

For a public trusted `Ffi.C.Layout` record used by value in a public foreign
signature, ABI-visible layouts include the stable producer record identity,
declaration-ordered closed fields, and the exact target/ABI canonical layout
catalog with full fingerprints required by ADR 0086. Loading reconstructs only
that public record schema in the isolated reference arena and verifies the full
catalog before consumer name resolution. It never merges dependency Modules or
exposes the projection as runtime reflection.

It excludes ordinary `internal`/`private` declarations and UDAs, runtime
reflection, compiler arenas, backend objects, and unrelated implementation
details. A public generic callable may carry one verified portable
specialization capsule containing its opaque transitive implementation closure.
Capsule-private identities are not entered into consumer name resolution and do
not widen visibility. Reference-only artifacts never execute. XML documentation
is separate and does not alter the public API hash. See ADR 0054.

Cross-Bubble declarations use
`SymbolIdentity { bubble: BubbleId, symbol: SymbolId }` under
[ADR 0036](./decisions/0036-typed-cross-bubble-function-references.md). The
first implementation emits public namespace functions with closed primitive
parameter/result signatures and effects. The generic extension uses a closed
recursive typed schema for parameters, bounds, aggregate/callable types, nominal
identities, and reserved built-in identities. Unsupported public signature or
capsule types reject metadata emission rather than becoming erased or dynamic.
HIR/MIR retain complete identities after any session-local metadata remapping.

For ADR 0095 checked casts, a public class reference additionally retains its
stable specialized direct-base identity, open/sealed fact, and exact specialized
interface witnesses. A consumer can therefore validate a visible
interface-to-class cast without loading source or discovering private types.
The linked implementation may retain private descendant ancestry for execution,
but it never enters consumer lookup or a runtime reflection surface. Matching is
by verified `SymbolIdentity` plus canonical arguments; session-local typed ID
remapping cannot replace the owning Bubble identity with a source name or path.

Portable capsule loading verifies the owner, schema, hash, type graph, effects,
dependencies, HIR invariants, visibility closure, and specialization budget.
Specialized identity derives from the source `SymbolIdentity` plus canonical
type arguments; loading never merges the dependency's Modules or namespace into
the consumer Bubble.

The version-1 capsule payload is encoded inline in canonical
`reference.metadata`. Its logical model is the already verified HIR/type
capsule; serialization cannot introduce names, types, effects, or dependencies.
The capsule and enclosing file have independent SHA-256 digests and explicit
resource counts. Unsupported or noncanonical encodings fail before the capsule
enters a consumer arena.

## Compile-time resolution algorithm

For selected Workspace/Package/Bubble roots, the toolchain:

1. reads and validates `bubble.toml` plus `bubble.lock`;
2. resolves exact Package sources and public library Bubbles;
3. checks editions, hashes, platform targets, PLRI ABI, and capabilities;
4. loads reference metadata into an isolated metadata arena;
5. indexes public namespaces/symbols and same-Bubble internal Modules;
6. resolves `using` and fully qualified names;
7. records exact `BubbleIdentity` dependencies in HIR/build metadata;
8. selects implementation artifacts only for linking or execution.

Reading reference metadata never initializes dependency code or enables runtime
reflection.

## Linking modes

### Static/native default

A binary Bubble closes its dependency graph and links library Bubble objects
into an executable/application-owned image. Unused code and metadata may be
dead-stripped.

### Shared native Bubble artifacts

An explicitly shared library Bubble exposes only a versioned Pop ABI. The
native loader verifies `bubble.manifest` before binding symbols. Platform
filenames remain implementation details behind `BubbleIdentity`. Cross-Bubble
calls use a stable public ABI thunk or whole-program resolution.

### VM Bubble artifacts

The future VM loads verified bytecode under the same logical Bubble manifest and
identity. Lazy VM slot linking must preserve native Bubble/type identity and
visibility semantics.

## Runtime Bubble contexts

Every application has one `BubbleContext` by default. It maps each requested
`BubbleIdentity` to one verified loaded instance and caches dependency results.

The default context:

- loads only the statically resolved graph;
- rejects identity/hash/ABI mismatches;
- initializes each Module once;
- detects initialization cycles and caches failures;
- shares one compatible Bubble instance across dependents;
- never probes ambient working directories.

Optional isolated contexts may later support plugins. Values cross contexts
only through explicitly shared ABI/interface contracts. Type identity includes
the load context, preventing unsafe casts between separately loaded Bubbles.

Unloadability is not promised initially. Unloading requires proof that no code,
object, callback, thread, coroutine, GC root, foreign handle, or retained
metadata references the Bubble.

## Initialization

Mapping a Bubble and initializing its Modules are distinct:

1. verify/map the artifact;
2. register private GC/dispatch metadata;
3. allocate Module state;
4. initialize dependency Bubbles in manifest order;
5. execute each Module initializer once;
6. publish the Bubble as `Ready`.

States are `Unloaded`, `Loading`, `Loaded`, `Initializing`, `Ready`, and `Failed`.
Failure remains cached in the context. Type/reference-only edges do not execute
runtime initialization.

## Versioning, security, and reproducibility

- Package resolution locks exact sources/versions for the Workspace.
- Bubble API/ABI hashes detect incompatible same-version artifacts.
- Generic portable bodies are versioned with HIR/MIR schemas.
- Manifests and all artifact files are hash verified.
- Load paths come from `bubble.lock`, not environment probing.
- Native foreign dependencies use the typed, target-specific, hash-verified
  `NativeLinkPlan` from ADR 0081. Artifacts and locks contain no ambient host
  paths, raw linker flags, shell commands, or runtime symbol lookups.
- Compile-time code can read permitted reference metadata but cannot execute
  dependency native code.
- Cache keys include Package/Bubble identities, content, API, target, edition,
  compiler, and PLRI versions.
- Two incompatible versions coexist only in isolated contexts or under distinct
  resolved identities.

## Diagnostics

Dependency/load diagnostics identify:

- source `using` or symbol use;
- current Package and Bubble;
- the direct `bubble.toml` dependency;
- the transitive `bubble.lock` resolution path;
- selected artifact and platform target.

“Namespace not found,” “Bubble not depended on,” “Package resolution conflict,”
and “Bubble implementation failed to load” are distinct errors with targeted
quick fixes.

## Influence boundary

The namespace/metadata/load-context design borrows from C#/.NET. Bubble,
Package, Workspace, target discovery, and lockfile workflow borrow from
Rust/Cargo. Pop Lang keeps Luau-shaped source, its own visibility rules,
strong-static/no-reflection architecture, MIR/LLVM/future-VM pipeline, compact
prelude, and non-OOP-first APIs.

Primary design references:

- [CLI, tooling, and units of code](./21-cli-tooling-and-code-units.md)
- [C# namespaces and using directives](https://learn.microsoft.com/en-us/dotnet/csharp/fundamentals/program-structure/namespaces)
- [.NET reference assemblies](https://learn.microsoft.com/en-us/dotnet/standard/assembly/reference-assemblies)
- [Cargo targets](https://doc.rust-lang.org/cargo/reference/cargo-targets.html)
- [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html)
