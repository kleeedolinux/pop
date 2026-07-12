<p align="center">
  <img src="assets/pop.png" alt="Pop Lang logo" width="140">
</p>

<p align="center">
  <strong>A statically typed, general-purpose language with the lightweight feel of Luau.</strong>
</p>

<p align="center">
  <a href="#why-pop">Why Pop?</a> •
  <a href="#why-not-luau">Why not Luau?</a> •
  <a href="#language-tour">Language tour</a> •
  <a href="#runtime">Runtime</a> •
  <a href="#project-status">Status</a>
</p>

# Pop Lang

Pop Lang is a native, strongly and statically typed programming language inspired by Luau.

It keeps the parts that make Luau pleasant to read and write: `end`-based blocks, lightweight syntax, local inference, first-class functions, closures, coroutines, and convenient collection literals.

The difference is in the foundation. Pop is designed from the beginning for general-purpose software, predictable native runtimes, portable backends, explicit APIs, and static guarantees without a dynamic fallback.

```luau
namespace Game.Players

public record Player
    name: String
    score: Int = 0
end

public function award(player: Player, points: Int): Player
    return player with {
        score = player.score + points,
    }
end
```

Pop is not a Lua compatibility layer and does not treat tables as the hidden implementation of every language feature. Records, classes, interfaces, modules, namespaces, arrays, and typed tables are separate concepts with separate semantics.

> [!IMPORTANT]
> Pop Lang is under active development and is not yet ready for production use.

## Why Pop?

Languages with lightweight syntax are often built around a dynamic runtime. Languages with strong static guarantees often introduce more syntax, more punctuation, and more ceremony.

Pop explores another direction:

- source code that stays visually close to Luau;
- every usable value and operation checked before execution;
- native records, classes, interfaces, closures, and collections;
- explicit integer and floating-point types;
- typed errors instead of exceptions as the normal failure path;
- a runtime designed for standalone applications, libraries, tools, services, games, and embedding;
- one language model that can be implemented by native compilers, interpreters, virtual machines, or transpilers.

The goal is not to remove complexity by hiding it at runtime. The goal is to give common programs a small, readable surface while keeping their behavior explicit enough for compilers, editors, runtimes, and developers to agree.

## Why not Luau?

Luau is an excellent language for its primary environment. It improves Lua with a fast type checker, inference, modern syntax, and a strong developer experience for embedded and game-oriented scripting.

Pop has a different goal.

A standalone Luau runtime can change where Luau executes, but it cannot remove the compatibility constraints of the language itself. General-purpose Luau implementations still need to account for features such as gradual typing, `any`, dynamic tables, metatables, Lua-style module values, and behavior inherited from Lua compatibility.

Those features are useful for scripting and migration, but they make it harder to guarantee that every operation has one statically known meaning across native compilation, embedded runtimes, tooling, and alternative backends.

This is also a risk seen in the JavaScript ecosystem. Better runtimes and TypeScript greatly improve the experience, but they cannot replace JavaScript's dynamic core. Large applications often accumulate extra layers for types, modules, builds, validation, packaging, and runtime compatibility, while dynamic escape hatches remain part of the system.

Pop starts on the other side of that trade-off:

- inference is preserved, but gradual fallback is rejected;
- table literals remain convenient, but tables are not records, classes, modules, or namespaces;
- metatable use cases become native language features or explicit typed protocols;
- modules and dependencies are statically analyzable;
- classes and closures use native runtime representations rather than hidden tables;
- foreign or unstructured data must cross an explicit typed boundary;
- new runtimes do not need to preserve Lua's dynamic object model.

Pop does not aim to clone Luau and then slowly remove its constraints. Each Luau feature is deliberately adopted, adapted, replaced, rejected, or deferred.

The architecture documentation maintains the complete, versioned Luau feature inventory.

## Language tour

### Static without constant annotation noise

Every expression, local, field, parameter, return value, collection element, and call target has a type before the program is executed.

Inference removes repetition:

```luau
local name = "Ana"
local score = 10
local active = true
```

But inference never becomes a dynamic fallback. Pop has no source-visible value like `any` or `dynamic` on which arbitrary operations are allowed.

External and unstructured data must be decoded into a known shape:

```luau
public record User
    id: UInt64
    displayName: String
end

public function decodeUser(json: JsonValue): Result<User, DecodeError>
    -- Validate the input and return a typed value.
end
```

### Familiar syntax, explicit semantics

Pop follows Luau's visual language:

- blocks end with `end`;
- functions use `function`;
- locals use `local`;
- annotations use `name: Type`;
- methods keep colon-call ergonomics;
- braces represent data and initializers, not executable declaration blocks;
- semicolons are not part of canonical source;
- namespace imports use `using`, not JavaScript-style destructuring.

```luau
namespace Studio.Gameplay

using Physics = Studio.Simulation.Physics

public function updatePlayer(player: Player, deltaTime: Float)
    Physics.step(player, deltaTime)
end
```

`using` only affects name resolution. It does not execute code, load a file, or create a runtime object.

### Records and tagged unions

Records are typed data, not tables with a convention:

```luau
public record Request
    path: String
    retryCount: Int = 0
end

local nextRequest = request with {
    retryCount = request.retryCount + 1,
}
```

Tagged unions represent states that must be handled explicitly:

```luau
public union LoadResult
    Ready(value: String)
    Missing
    Failed(message: String)
end

public function printResult(result: LoadResult)
    match result
    when LoadResult.Ready(value) then
        Io.print(value)
    when LoadResult.Missing then
        Io.print("missing")
    when LoadResult.Failed(message) then
        Io.print(message)
    end
end
```

The initial `match` form is exhaustive. Adding a new union case requires callers to decide how it should be handled.

### Native classes when identity matters

Pop is data-first, not object-oriented-first. Records, unions, functions, and composition should handle most application code.

Classes are available when a value needs identity, encapsulated mutable state, a lifecycle, or runtime polymorphism:

```luau
public class Connection
    private closed: Boolean = false

    public function Connection:close()
        if not self.closed then
            self.closed = true
            -- Release the owned transport.
        end
    end
end
```

A class is not a table with a metatable. Its fields, visibility, layout, methods, and implemented interfaces are known by the compiler and runtime.

### Typed collections

Pop keeps Luau-like collection syntax while giving each collection a real type:

```luau
local names: {String} = { "Ana", "Bruno" }

local scores: {[String]: Int} = {
    ana = 10,
    bruno = 12,
}
```

`{T}` is an array type. `{[K]: V}` is a typed associative table. Heterogeneous collections require an explicit union or shared interface.

Tables do not double as modules, records, classes, namespaces, tuples, or universal objects.

### Explicit numeric types

Pop does not use one universal IEEE-754 `number` type.

The language includes fixed-width signed and unsigned integers, `Float32`, and `Float64`. Common aliases include:

| Alias | Type |
| --- | --- |
| `Int` | `Int64` |
| `Float` | `Float64` |
| `Byte` | `UInt8` |

This makes storage, arithmetic, overflow behavior, foreign interfaces, and cross-platform execution easier to reason about.

### Typed failures

Expected failures use typed values similar to `Result<T, E>`:

```luau
public function loadConfiguration(path: String): Result<Configuration, LoadError>
    -- ...
end
```

`panic` is reserved for broken invariants or unrecoverable internal failures. It is not the ordinary application error API.

## Runtime

Pop is designed as a general-purpose language, so the runtime is part of the language model rather than an afterthought attached to a scripting host.

The runtime is responsible for consistent behavior across implementations, including:

- memory management;
- strings and collections;
- closures and captured state;
- classes and interface dispatch;
- coroutines, suspension, and task integration;
- panic, cleanup, and runtime traps;
- foreign-function boundaries;
- platform and standard-library services.

### Managed memory

The production runtime design uses a precise concurrent generational garbage collector.

In practical terms:

- short-lived allocations can be collected efficiently in a moving nursery;
- mature objects remain mostly non-moving;
- the collector knows the exact location of managed references;
- native stack frames and runtime structures use precise roots and stack maps;
- write barriers preserve correctness across generations and concurrent work.

Pop does not expose collector-specific object headers as part of the language. Resource cleanup should use explicit ownership and lifecycle APIs rather than depending on when garbage collection happens.

### Native runtime values

Language features use runtime representations that match their semantics:

- records are records;
- classes have declared layouts and native method dispatch;
- closures use typed environments;
- arrays and tables are distinct collection types;
- interfaces expose only their declared operations;
- namespaces and modules do not become runtime tables.

This gives runtimes freedom to optimize without preserving a universal dynamic table representation.

### Portable backends

Pop's canonical MIR is backend-agnostic. It describes typed control flow and runtime operations without tying the language to LLVM, a specific virtual machine, or one garbage collector.

That allows implementations to consume the same semantics for different execution strategies, including:

- native machine-code compilation;
- a reference interpreter;
- a virtual machine or JIT;
- transpiling backends;
- sandboxed embedded runtimes;
- specialized platform backends.

A backend may choose a different execution strategy, but it must not silently redefine language behavior.

## Modules, packages, and workspaces

Each `.pop` source file is a Module inside one file-scoped namespace.

```luau
namespace Game.Players
```

Declarations have explicit visibility:

- `public` is available to dependent libraries;
- `internal` is available inside the same independently compiled unit;
- `private` is limited to the current file.

Omitted namespace-level visibility defaults to `internal`.

Projects use `bubble.toml` for package metadata and dependencies. Pop's ownership model is:

```text
Item → Module → Bubble → Package → Workspace
```

A **Bubble** is an independently compiled dependency and the boundary for `internal` visibility. A **Package** is a versioned, publishable directory. A **Workspace** groups packages under one resolver, lockfile, cache, and policy root.

A conventional project layout is:

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

Modules and namespaces are compile-time concepts. They are not values returned from `require`, and dependency loading cannot be computed from arbitrary runtime strings.

## Tooling direction

Pop is designed around one coherent toolchain for common development tasks:

```text
pop check
pop build
pop run
pop test
pop format
pop lint
pop add
pop remove
```

Diagnostics, symbols, metadata, documentation, and automated fixes are intended to be available through stable structured formats, so editors and build tools do not need to scrape terminal output.

The language formatter owns canonical whitespace and layout. Pop source uses four-space indentation, avoids mandatory semicolons, and aims for one obvious readable shape.

## Relationship to Luau

Pop prefers familiar Luau syntax whenever the semantics truly match. Familiarity does not override the language model.

Examples of the current direction:

| Luau area | Pop direction |
| --- | --- |
| Local inference | Adopt, without gradual fallback |
| Type annotations and narrowing | Adopt and strengthen |
| First-class functions and closures | Adopt with native typed environments |
| Coroutines | Adapt to backend-neutral runtime operations |
| Arrays and table literals | Adopt the ergonomics with static element types |
| Tables as records or objects | Replace with records, classes, interfaces, arrays, and typed tables |
| Metatables and metamethods | Replace common cases with native constructs and typed protocols |
| `require`-style module values | Replace with namespaces, package dependencies, and analyzable initialization |
| Implicit globals | Reject |
| One universal `number` type | Replace with explicit integer and floating-point types |
| `any` and dynamic escape behavior | Reject |
| Sandboxed embedding | Preserve as a deployment capability |

The full feature inventory is versioned because Luau continues to evolve. A feature is not silently forgotten: it must be adopted, adapted, replaced, rejected with a reason, or deliberately deferred.

## Design principles

- Keep the common case small and readable.
- Prefer data and functions before classes and inheritance.
- Preserve Luau-shaped syntax when its meaning remains accurate.
- Give every usable value and operation a static type.
- Keep dynamic or foreign data behind explicit typed boundaries.
- Make modules, packages, and visibility analyzable before execution.
- Keep runtime behavior consistent across conforming backends.
- Do not reintroduce Lua's table-centered semantics through compatibility shortcuts.

## Project status

Pop Lang is under active development and is not yet a stable production language.

The repository currently contains the language design, accepted architecture decisions, compiler and runtime foundations, tooling work, and conformance tests. Syntax and runtime behavior may still change as implementation milestones are completed.

The architecture documents are the current source of truth:

- [Architecture overview](architecture/README.md)
- [Syntax and nomenclature](architecture/13-syntax-and-nomenclature.md)
- [Implementation roadmap](architecture/07-implementation-roadmap.md)
- [Accepted decisions](architecture/decisions/README.md)

## Repository layout

```text
architecture/       Language, runtime, compiler, and tooling design
architecture/decisions/
                    Accepted Architecture Decision Records
crates/compiler/    Syntax, resolution, types, compile time, HIR, MIR, drivers
crates/extensions/  Independent Pop.Data/Ai/Cli/Rpc/Syntax/Lsp package builds
crates/runtime/     PLRI and bootstrap/native runtime contracts
crates/tools/       Architecture tests, formatter, documentation, CLI tooling
libraries/internal/ Pop.Internal bootstrap foundations
libraries/standard/ Pop.Standard bootstrap foundations
```

The compiler and runtime are implemented in Rust. Rust is the implementation language of the toolchain; it does not define Pop source syntax or its package model.

## License

Pop Lang is released under the [MIT License](LICENSE).
