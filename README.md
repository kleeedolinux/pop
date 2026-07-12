<p align="center"><img src="assets/pop.png" alt="pop" width="120"></p>
<p align="center">
  <a href="#status">Status</a> •
</p>

# Pop Lang

Pop Lang is a native, strongly and statically typed programming language
inspired directly by Luau. It keeps Luau's lightweight syntax, readable
`end`-delimited blocks, local inference, first-class functions, closures,
coroutines, and table/array literal ergonomics while giving every language
abstraction explicit static semantics.

Pop Lang is designed for fast tooling, predictable compilation, portable
backends, and APIs built from data, functions, composition, and small nominal
interfaces. It is not a dynamically typed Lua compatibility layer and it is not
an object-oriented-first language.

## Status

This repository is architecture-first. The documents under [`architecture/`](architecture/)
are the binding project contract, and the Rust workspace contains the compiler,
runtime, tooling, and conformance-test foundations described there. The language
and its implementation are being developed in vertical, testable milestones;
the implementation roadmap is in [`architecture/07-implementation-roadmap.md`](architecture/07-implementation-roadmap.md).

The accepted architecture is deliberately evolvable, but implementation
convenience does not override it. A cross-cutting semantic change requires an
accepted Architecture Decision Record, synchronized documentation, and
conformance tests.

Pop Lang is released under the [MIT License](LICENSE).

## A small example

Pop Lang preserves a familiar Luau shape while making data and visibility
explicit:

```luau
namespace Game.Players

private const INITIAL_SCORE = 0

public record Player
    name: String
    score: Int = INITIAL_SCORE
end

public function award(player: Player, points: Int): Player
    return player with {
        score = player.score + points,
    }
end
```

A tagged union and exhaustive match use resolved type and case identities:

```luau
namespace Game.Results

public union LoadResult
    Ready(value: String)
    Missing
end

public function message(result: LoadResult): String
    match result
    when LoadResult.Ready(value) then
        return value
    when LoadResult.Missing then
        return "missing"
    end
end
```

The syntax is intentionally low-ceremony: braces are data or initializer
literals, not executable declaration blocks; semicolons and JavaScript-style
`import`/`export` syntax are not part of canonical Pop Lang source.

## Core principles

- Every runtime value and operation has a compiler-proven static type.
- Inference fills in types; it never becomes a dynamic fallback.
- There is no source-visible `Any`, `Dynamic`, unchecked member lookup, or
  runtime call-by-name operation.
- Records, tagged unions, tuples, arrays, typed tables, modules, namespaces,
  classes, Bubbles, and Packages are distinct concepts.
- HIR and MIR are backend-neutral. LLVM is a backend, not the compiler's
  semantic source of truth.
- Conforming backends must agree on language behavior.
- Runtime services are reached through the versioned Pop Lang Runtime
  Interface (PLRI).
- Compile-time execution is deterministic, budgeted, capability-limited, and
  unable to parse or inject source text.
- Runtime reflection is absent by default; retained metadata requires an
  explicit typed adapter boundary.
- Reintroducing Lua's dynamic/table-centered architecture is a release-blocking
  Lua regression.

The full rationale and invariant list are maintained in the
[architecture overview](architecture/README.md) and the
[architecture conformance policy](architecture/19-architecture-conformance-and-regression-policy.md).

## Language model

### Static types

The initial type system includes fixed-width integers (`Int8` through `Int64`
and `UInt8` through `UInt64`), IEEE floating-point types (`Float32` and
`Float64`), `Boolean`, `String`, `nil` through optional types, tuples, arrays,
typed tables, records, tagged unions, classes, interfaces, and fully typed
function values.

`Int` is the platform-independent `Int64` alias. `Float` is `Float64`, and
`Byte` is `UInt8`. Integer overflow and division by zero are checked/default
traps; explicit wrapping operations are separate semantics.

Expected recoverable failures use typed `Result<T, E>`-style values. `panic`
represents an invariant failure and unwinds through verified cleanup edges;
exceptions are not the ordinary error API.

### Data-first abstractions

Pop Lang encourages the following order of design:

1. local values and plain functions;
2. records and tagged unions;
3. arrays, typed tables, and generic algorithms;
4. namespaces and Modules;
5. composition and function/capability values;
6. small nominal interfaces at real polymorphic boundaries;
7. classes where identity, mutable lifecycle, or runtime dispatch is required.

Classes support deliberate single implementation inheritance and multiple
interface implementation, but inheritance is not the default way to organize
an API. Ordinary modules do not return public symbol tables, and classes,
records, and namespaces are not secretly universal runtime tables.

### Naming and visibility

Canonical Pop Lang naming is part of the language contract:

| Entity | Convention | Example |
| --- | --- | --- |
| Namespace, type, interface, Package, Bubble | `PascalCase` | `Game.Players`, `Player` |
| Function, method, field, local, parameter, Module filename | `camelCase` | `loadPlayer`, `displayName` |
| Constant | `UPPER_SNAKE_CASE` | `MAX_RETRIES` |
| Attribute | `PascalCase` | `@Serializable` |

Every namespace-scope declaration explicitly uses `public`, `internal`, or
`private`. `public` crosses a Bubble boundary through reference metadata;
`internal` is visible inside one Bubble; `private` stops at the declaring
Module. There is no `export` prefix, export list, or implicit namespace
visibility.

`using` changes compile-time name lookup only. It does not load code, create a
runtime value, forward visibility, or create a dependency by itself.

## Compile-time programming and UDAs

User-defined attributes (UDAs) are nominal, typed, immutable compile-time
values. Their arguments are checked like constant constructor arguments, and
their attachments are validated against explicit target permissions and
repeatability rules.

```luau
@AttributeUsage(
    targets = { AttributeTarget.Record, AttributeTarget.Field },
    repeatable = false,
)
public attribute Serializable(version: UInt32 = 1)
```

Typed queries use compiler-owned identities rather than strings:

```luau
private const HAS_SERIALIZABLE = hasAttribute<<Serializable>>(Player)
private const PLAYER_METADATA = attribute<<Serializable>>(Player)
```

Compile-time values may contain immutable scalars, tuples, records,
homogeneous arrays, enum/union values, and resolved compiler-owned handles.
Compile-time evaluation cannot access ambient files, network, environment,
clock, process state, randomness, backend objects, or source parsing APIs.

See [UDAs, compile time, and reflection](architecture/10-udas-compile-time-and-reflection.md)
and [ADR 0023](architecture/decisions/0023-source-integrated-uda-contract.md).

## Compiler architecture

The required semantic pipeline is:

```text
Source
  → tokens
  → lossless syntax tree
  → declaration index
  → resolved AST
  → typed / compile-time analysis
  → HIR
  → canonical MIR
  → backend
```

HIR preserves typed language concepts, resolved stable IDs, closures,
interfaces, matches, calls, and source origins. MIR makes control flow,
evaluation order, calls, effects, failure edges, safe points, roots, barriers,
allocation, and runtime operations explicit.

The MIR interpreter and native LLVM backend consume canonical MIR. A backend
does not call back into parsing, name resolution, type checking, or compile-time
evaluation. The HIR/MIR boundary is what permits a future VM backend without
reconstructing source-level meaning.

The principal compiler crates are:

- `pop-syntax`: lossless syntax and source parsing;
- `pop-resolve`: declaration indexing, names, namespaces, and visibility;
- `pop-types`: semantic types, body checking, interfaces, matches, effects, and
  source-integrated attribute contracts;
- `pop-compile-time`: restricted typed compile-time lowering and evaluation;
- `pop-hir`: resolved, typed, backend-neutral HIR;
- `pop-mir`: canonical MIR, verification, text form, and portable optimization;
- `pop-backend-mir-interp`: reference execution and differential behavior;
- `pop-backend-llvm`: native backend boundary;
- runtime crates: PLRI, native bootstrap services, allocation, roots, barriers,
  and precise GC contracts.

## Effects, failures, and memory

Function types, HIR functions, MIR functions, and call sites carry closed effect
summaries. The initial summary records allocation, managed-reference mutation,
traps, panic/unwind, suspension, unsafe memory, FFI, ambient I/O, compiler
queries, GC safe points, and root operations. There is no unknown or dynamic
effect fallback.

Calls through function values consume the function type's closed summary.
Interface implementations must satisfy the exact declared member summary and
cannot widen it. Recursive call-graph components are solved to a least fixed
point.

The production GC design is a precise concurrent generational collector with a
moving nursery, mostly non-moving mature heap, precise roots and stack maps,
SATB and generational barriers, and bounded pause work. The bootstrap runtime
is intentionally smaller but preserves the same precise map/root/barrier
contracts needed by the later collector.

## Packages, Bubbles, and Workspaces

Pop Lang uses the ownership hierarchy:

```text
Item → Module → Bubble → Package → Workspace
```

- An **Item** is a declaration or member with a stable identity.
- A **Module** is one `.pop` file and the `private` boundary.
- A **Bubble** is an independently compiled dependency and `internal` boundary.
- A **Package** is a publishable, versioned directory containing `bubble.toml`.
- A **Workspace** groups Packages under one resolver, lockfile, cache, and
  policy root without merging their visibility.

The conventional layout is:

```text
bubble.toml
src/
    lib.pop
    main.pop
    bin/
tests/
examples/
benchmarks/
```

Workspaces share a deterministic `bubble.lock` and `target/` output/cache root.
Paths are resolution inputs, never semantic identity. Library Bubbles emit
self-describing `.poplib` artifacts with manifests, public reference metadata,
documentation, hashes, target capabilities, and exact Bubble dependencies.

The detailed unit and loading contract is in
[CLI, tooling, and units of code](architecture/21-cli-tooling-and-code-units.md)
and [Bubbles, namespaces, artifacts, and loading](architecture/14-libraries-namespaces-and-loading.md).

## Tooling

The unified user-facing command is `pop`. Canonical commands include:

```text
pop check
pop build
pop run
pop test
pop benchmark
pop documentation
pop format
pop lint
pop fix
pop add
pop remove
pop update
pop tree
pop metadata
pop package
pop publish
pop install
pop clean
```

`pop` remains the language, Workspace, Package, Bubble, documentation, and
package-manager command. `pop install` builds and installs a Package's public
binary Bubble. The narrowly separate `popup` command manages complete Pop Lang
compiler/runtime toolchain distributions; it never edits `bubble.toml` or
`bubble.lock` and is not a second Package manager.

The accepted `popup` workflow includes deterministic noninteractive commands
for listing, installing, selecting, updating, diagnosing, and removing exact
toolchains, plus an optional Ratatui view over the same typed operations:

```text
popup list --available
popup install stable
popup default stable
popup run --toolchain 1.0.0 -- pop check
popup doctor
```

Each toolchain is an immutable relocatable host distribution containing one
compatible compiler, runtime/PLRI, first-party tools, `Pop.Internal`,
`Pop.Standard`, target support, licenses, and versioned file inventory. The
managed `pop` shim selects an already installed exact distribution using an
explicit `popup run`, `POPUP_TOOLCHAIN`, the nearest exact
`pop-toolchain.toml`, then the global default. Checked-in pins record an exact
version and distribution digest; selection never downloads implicitly.

`popup` discovers releases through a canonical versioned release index signed
by the trusted distribution root, not by scraping repository tags, pages,
branches, or filenames. Signature, digest, expiry, rollback, target, archive,
and inventory checks fail closed. Installation and self-update stage and verify
complete content before atomic activation, preserving the previous usable state
after interruption or concurrent access.

The one-script Bash bootstrap is only a narrow path to an exact pinned `popup`:
it verifies the documented host, size, and embedded SHA-256 digest before
execution, and does not evaluate downloaded text, build the repository, invoke
Cargo, request `sudo`, or silently edit shell startup files. The release gate
requires reproducible relocatable archives, signed metadata,
licenses/SBOM/provenance, cross-backend/runtime/foundational-Bubble checks, and
an empty-root smoke installation outside the checkout before a signed channel
is published. See
[ADR 0028](architecture/decisions/0028-toolchain-distribution-and-popup-management.md).

Machine tooling consumes versioned structured diagnostics, metadata, symbols,
build events, and workspace edits. It does not scrape human CLI output.
Diagnostics use stable `POP####` codes, typed arguments, typed
source/manifest/path/artifact/toolchain-state/no-location locations, labels,
notes, origins, severity/category, warning waves, and semantic quick fixes.
Toolchain failures use non-suppressible `POP9xxx` codes without fabricated
source spans. Plain, versioned JSON, and Ratatui presentation preserve the same
typed facts; color and widget layout are not machine APIs. Safe fix-all
operations are atomic, version-checked, composable, formatted, and
postcondition-verified.

Public documentation uses Lua-shaped `---` comments with checked XML concepts.
Documentation parameters, type parameters, returns, typed errors, effects,
allocation, thread safety, complexity, and `cref` links are validated and
emitted separately from runtime metadata.

## Repository layout

```text
architecture/       Accepted language, compiler, runtime, and tooling contract
architecture/decisions/
                    Accepted Architecture Decision Records
crates/compiler/    Syntax, resolution, types, compile time, HIR, MIR, drivers
crates/runtime/     PLRI and bootstrap/native runtime contracts
crates/tools/       Architecture tests, formatter, documentation, CLI tooling
libraries/internal/ Pop.Internal bootstrap foundations
libraries/standard/ Pop.Standard bootstrap foundations
```

The workspace uses Rust edition 2024 for the implementation. Rust is the
host language of the toolchain; it does not define Pop Lang's source syntax or
replace Pop Lang's Package/Bubble model.

## Roadmap

The implementation roadmap proceeds through vertical slices:

1. decisions, workspace skeleton, syntax, diagnostics, and bootstrap metadata;
2. typed front end, HIR, UDAs, compile-time evaluation, and project discovery;
3. canonical MIR, interpreter, native classes, collections, interfaces,
   exhaustive matches, closures, and precise bootstrap GC;
4. LLVM native code generation, PLRI integration, artifacts, and differential
   backend testing;
5. generics, error handling, coroutines/async, FFI, retained metadata, mature
   concurrent GC, and broader standard-library profiles.

Every milestone requires deterministic positive and negative tests, textual IR
fixtures, architecture traceability, documentation updates, and permanent
regressions for forbidden dynamic/table-based behavior.

## Architecture reference

Start with these documents when changing or extending the project:

- [Architecture overview](architecture/README.md)
- [Compiler pipeline](architecture/03-compiler-pipeline.md)
- [Intermediate representations](architecture/04-intermediate-representations.md)
- [Type-system architecture](architecture/12-type-system-architecture.md)
- [Syntax and nomenclature](architecture/13-syntax-and-nomenclature.md)
- [Architecture conformance and regression policy](architecture/19-architecture-conformance-and-regression-policy.md)
- [Toolchain distribution and `popup` management](architecture/decisions/0028-toolchain-distribution-and-popup-management.md)
- [Accepted decisions](architecture/decisions/README.md)

The project treats those documents, accepted ADRs, specifications, conformance
tests, and implementation as one traceable contract. When they disagree, the
disagreement must be repaired; code does not silently redefine the language.
