# Paradigm and API Style

## Position

Pop Lang is a strongly typed procedural, functional, and data-oriented language
with optional native object-oriented tools. It is not “everything is an object,”
and classes are not the default unit of organization.

The existence of native classes solves a semantic/runtime problem left vague by
Lua tables. It does not require Pop Lang programs or libraries to model every
operation as a method or every subsystem as a class hierarchy.

## Preferred abstraction order

Choose the first tool that correctly expresses the problem:

1. local values and plain functions;
2. records and tagged unions;
3. arrays/tables plus generic algorithms;
4. Modules and namespaces;
5. composition of functions/data/capabilities;
6. a small nominal interface for a real polymorphic boundary;
7. a class for stable identity, encapsulated mutation/lifecycle, or runtime
   dispatch;
8. inheritance only when substitutability and shared implementation are both
   intentional long-term contracts.

Moving down this list needs a reason. “To organize methods” is not enough reason
to introduce a class.

## Data first

### Records

Records are the ordinary product/data type. Their field bindings are immutable
by default, they support structural equality when every field type supports it,
and they update through a typed `with` expression. This is shallow: a record can
contain an explicitly mutable `List`/`Table`, whose contents retain their own
semantics.

```luau
public record Position
    x: Float
    y: Float
end

public function move(position: Position, dx: Float, dy: Float): Position
    return position with {
        x = position.x + dx,
        y = position.y + dy,
    }
end
```

The optimizer may update/copy fields in place when uniqueness/escape analysis
proves the change unobservable. Source semantics remain value-oriented.

Records are preferred for messages, configuration, requests/responses, AST/HIR
nodes, coordinates, errors, serialized data, and component state.

### Tagged unions

Tagged unions model alternatives/state transitions without inheritance:

```luau
public union LoadState
    Idle
    Loading(progress: Float)
    Ready(data: Bytes)
    Failed(error: LoadError)
end
```

Matches must be exhaustive. Adding a case produces targeted compile diagnostics
and a quick fix to add missing branches.

Use unions instead of base classes whose subclasses exist only to carry
different data.

### Collections and data-oriented layout

Collections are typed values operated on by functions:

```luau
List.push(players, player)
List.sort(players, byScore)

local alive = Sequence.filter(players, isAlive)
local scores = Sequence.map(alive, playerScore)
```

Game/simulation code can store components by type/ID rather than allocating one
object graph per entity:

```luau
internal record World
    positions: Table<EntityId, Position>
    velocities: Table<EntityId, Velocity>
end

internal function updateMotion(world: World, elapsed: Time.Duration): World
    -- Transform component data in batches.
end
```

The standard library should support contiguous arrays, views, stable iteration,
bulk transforms, and allocation-aware algorithms so data-oriented code does not
need class wrappers.

## Functions first

Plain functions are declared directly at namespace/Module scope with
explicit `public`, `internal`, or `private` visibility:

```luau
namespace Image

public function resize(image: ImageData, width: Int, height: Int): ImageData
end

public function encode(image: ImageData): Result<Bytes, EncodeError>
end
```

Pop Lang does not require a static `ImageService`, `ImageManager`,
`ImageFactory`, singleton object, or module table. Namespace context already
organizes these functions.

### Function composition

Functions and closures are first-class. Generic algorithms accept functions
instead of strategy objects when no persistent identity is needed.

Prefer:

```luau
local visible = Sequence.filter(players, canSee)
local names = Sequence.map(visible, displayName)
```

over constructing filter/mapper objects or fluent chains whose intermediate
allocation/evaluation is unclear.

Pop Lang can add carefully designed pipeline syntax later, but the base library
does not require it for composable function APIs.

## Namespace and type companions

A type can have a companion function namespace without becoming a class/static
object. `List.push(list, value)` resolves `List` statically as a type companion.
No `List` runtime object or static constructor exists.

Companion functions are useful when the type supplies important context:

- `List.push`;
- `Table.keys`;
- `Result.ok` / `Result.error`;
- `Json.encode`;
- `Text.slice`.

Do not repeat that context in long names. Within `Http`, prefer `Request` over
`HttpRequest`; within `Json`, prefer `Value` over `JsonValue`. A stateful noun
such as `Client` is accepted only for an opaque resource with real protocol
state, such as a connection pool.

## Interfaces

Interfaces exist for true reusable behavior boundaries:

- iteration;
- equality/order/hash protocols;
- reader/writer capabilities;
- backend/driver boundaries;
- dependency injection at application architecture boundaries when justified.

Rules:

- keep interfaces small and nominal;
- compose capabilities instead of creating deep interface inheritance;
- do not use an `I` prefix;
- do not create marker interfaces—use typed PascalCase attributes;
- do not create an interface for one implementation without a testing/plugin/
  architectural boundary;
- prefer a function parameter when behavior is a single operation;
- avoid mutable property bags in interfaces;
- adding a required member is a breaking public-Bubble API change.

Function values frequently replace single-method interfaces:

```luau
internal type Predicate<T> = function(value: T): Boolean
```

## Classes

Classes are appropriate for:

- actors/entities/resources with stable identity;
- encapsulated mutable invariants;
- scheduler/runtime/driver objects with lifecycles;
- foreign/native handles that must own cleanup state;
- runtime polymorphism where a nominal interface alone is insufficient;
- framework extension points intentionally designed for inheritance.

Classes are sealed by default. `open class` and overridable members are explicit.
Single inheritance is available, but composition should be considered first.

Classes are usually not appropriate for:

- DTOs/messages/configuration;
- errors or variant cases;
- collections or algorithm namespaces;
- stateless services/helpers/managers;
- a wrapper around one function;
- factories that only choose a union case/record constructor;
- fluent builders when a record literal/namespace function is clearer.

## Methods

Methods are allowed on classes and remain Luau-like with colon syntax. They are
appropriate when behavior is inseparable from an identity/lifecycle invariant.

Do not turn every free function whose first parameter is a value into a method.
In particular, cross-cutting algorithms remain free/generic functions so they do
not bloat core type APIs.

The standard library favors:

```luau
Text.slice(value, start, finish)
List.sort(values, compare)
Json.encode(value)
```

over:

```luau
-- Discouraged API shapes
value:slice(start, finish)
values:sort(compare):map(transform):toJson()
```

This keeps dispatch, allocation, evaluation order, and module ownership clear.

## Construction

- Records/unions use typed literals/cases.
- Classes use native constructors only when identity/lifecycle initialization is
  needed.
- Factory functions are plain namespace functions when construction involves
  parsing, I/O, caching, or selecting among result variants.
- Incremental stateful construction uses a direct reusable buffer or domain
  value only when a record/list cannot express it; for text, prefer a
  `Bytes.Buffer` with an explicit checked UTF-8 finish operation.

## Errors and effects

Expected failure is data:

```luau
public union FileError
    Missing(path: Io.Path)
    Denied(path: Io.Path)
    InvalidData(message: String)
end

public function load(path: Io.Path): Result<Bytes, FileError>
end
```

There is no error base-class hierarchy required for routine failure. `panic`
remains for broken invariants.

Effects such as async, blocking, allocation, unsafe memory, and compile-time
eligibility are static summaries/attributes, not methods on context objects.

## Dependency injection and services

The language/standard base does not ship an OOP service-container pattern as a
foundation.
Dependencies should normally be explicit function parameters, records of
capabilities, or small interfaces at real dynamic boundaries.

Prefer:

```luau
internal record UserOps
    load: function(id: UserId): Result<User, UserError>
    save: function(user: User): Result<(), UserError>
end
```

over a container resolving opaque service objects by runtime type/name.

## Standard-library naming and import pressure

The fixed `Pop` prelude exposes high-frequency types and child namespace names.
Common standard code therefore needs no `using`:

```luau
local bytes = Json.encode(player)
local fileResult = File.read(path)
local delay = Time.seconds(2)
local task = Task.spawn(loadPlayer)
```

Context keeps names short:

- `Json.Value`, not `JsonValue`;
- `Http.Request`, not `HttpRequest`;
- `Bytes.Buffer`, not `ByteBufferBuilder`;
- prelude `CancelToken`, not `Async.CancellationToken`;
- `Set<T>`, not `HashSet<T>`;
- `Table<K, V>`, not `Dictionary<K, V>`.

External/user libraries remain explicit with `using`; the language does not
allow projects to inject arbitrary global usings.

## Concision and cost review

API review evaluates real call sites, not only declarations. Each public family
shows a concise default call, advanced typed options, and an efficient
view/buffer/stream/resource form. Common operations normally require one call;
advanced control must not replace the common path with a builder chain.

Short names come from stable domain context, not arbitrary truncation. Review
rejects repeated words (`File.openFile`), vague containers (`System.Manager`),
and generic action nouns (`Utility`, `Helper`). It also rejects convenience that
hides materialization, copying, task scheduling, dynamic/interface dispatch, or
native transitions. ADR 0032 and section 22 define the full cost contract.

## Analyzer guidance

The `ApiDesign` warning group may report review-level suggestions for public
APIs:

- class with only static/stateless members;
- public class that only stores data and could be a record;
- inheritance used only for variant data;
- redundant namespace/type prefixes;
- single-method interface replaceable by a function value;
- factory/service/helper/manager class with no state/lifecycle;
- fluent API with hidden allocation/blocking;
- common operation requiring avoidable construction/configuration ceremony;
- undocumented allocation, copying, dispatch, suspension, or native boundary;
- marker interface replaceable by an attribute.

These are warnings with semantic quick fixes/previews where safe, not hard
language errors. Legitimate identity/lifecycle designs can document/suppress the
specific warning.

## Review checklist

Before adding a class to the language, compiler API, or standard library:

1. Why is a record/union plus functions insufficient?
2. Is stable identity observable and necessary?
3. Which invariant requires encapsulated mutation?
4. Is runtime dispatch required, or would generics/function values work?
5. Can composition replace inheritance?
6. Does the type repeat namespace context in its name?
7. Can the API avoid forcing imports for a common standard operation?
8. Are allocation, dispatch, blocking, and cleanup visible?
