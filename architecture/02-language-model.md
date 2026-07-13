# Language Model

This document defines semantic direction and canonical syntax examples. The
complete grammar belongs to the language specification.

## Strong static type model

Pop Lang is strongly and statically typed. Every expression, local, field,
parameter, return value, collection element, and call target has a type known
before MIR is built. An omitted annotation requests inference, not dynamic
typing.

The initial type families include:

- the `nil` literal/type case, `Boolean`, fixed-width integer types,
  floating-point types, and `String`;
- tuples and function types;
- structural records;
- nominal classes and interfaces;
- generic types and constrained type parameters;
- unions, optional types, singleton types, and flow narrowing;
- typed arrays and tables;
- opaque nominal handles for resources whose representation is hidden;
- `Never` for expressions that do not return;
- an internal `error` type used only to recover after a diagnostic.

There is no user-visible `dynamic`, `any`, or unknown value on which operations
can be performed. A top type may exist for type-theory purposes only if values
must be narrowed to a concrete supported type before use.

### Static boundaries

- JSON and similar data are decoded into a declared schema, a tagged data tree,
  or a typed result.
- Foreign pointers use explicit typed wrappers and unsafe operations.
- Existential/interface values expose only the operations in their static
  interface.
- Downcasts, when supported, target a named type and return an optional/result;
  they do not produce a dynamically typed value.
- Heterogeneous collections use an explicit union or interface element type.

### Numeric source semantics

Decimal-point and base-ten-exponent literals are floating-point values. An
expected `Float32` or `Float64` type selects their format; without one they use
`Float` (`Float64`). They never implicitly become integers.

Numeric values convert explicitly through target-type call syntax such as
`Float64(count)` and `Int32(total)`. These forms are compiler-known typed
conversions, not ordinary overloads or runtime type-name lookup. Integer target
conversions and float-to-integer conversions are checked and use the closed
`NumericConversion` trap for invalid or out-of-range values. Ordering includes
`<`, `<=`, `>`, and `>=`; IEEE ordering comparisons with NaN are false. See
ADR 0040.

## Luau-first syntax rule

Pop Lang starts from Luau grammar and vocabulary. Additions should reuse Luau
conventions:

- block constructs end with `end`;
- functions use `function`;
- locals use `local`;
- annotations use `name: Type` and `function(...): ReturnType`;
- methods retain colon-call ergonomics;
- braces remain data literals, not mandatory declaration blocks;
- semicolons are not required;
- namespace imports use semicolon-free `using`, not JavaScript destructuring.

Syntax that merely resembles another language should not be added when a
Luau-shaped form expresses the same semantics clearly.

## Default program structure

Most Pop Lang code should be expressed as data plus functions:

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

The `with` expression creates an updated record value and preserves static field
checking. Records are not tables and do not carry hidden method dictionaries.
Tagged unions and exhaustive matching model state alternatives without base
classes.

## Classes are native but exceptional

A class owns nominal identity, declared fields, methods, visibility,
construction rules, and layout. It is not a table with a metatable.

Canonical direction:

```luau
namespace Studio.Transport

public class Connection
    public endpoint: Endpoint
    private closed: Boolean = false

    public function Connection.new(endpoint: Endpoint): Connection
        return Connection {
            endpoint = endpoint,
        }
    end

    public function Connection:close()
        if not self.closed then
            self.closed = true
            -- Release the owned transport.
        end
    end
end
```

Construction keeps Luau's keyed-data beauty while producing a native class
instance with required-field and visibility checks. It is not a table literal.

A class is justified when at least one of these matters:

- stable object identity;
- encapsulated mutable invariants across operations;
- a lifecycle/resource owner;
- explicit virtual/interface dispatch;
- framework/runtime interoperation requiring a nominal reference object.

Data transfer, configuration, parser trees, messages, errors, options, and most
business data should normally be records/unions instead.

The semantic model supports:

- fixed-offset access for declared fields;
- direct dispatch for statically known methods;
- explicit virtual/interface dispatch where polymorphism requires it;
- separate static/class members;
- single implementation inheritance for explicitly `open` classes;
- multiple nominal interface implementation;
- composition without runtime table synthesis;
- no undeclared instance fields;
- no member lookup by runtime string.

Classes do not form an implicit root hierarchy. There is no `Object` type and no
automatic methods such as `toString`, equality, or hashing inherited by every
value.

## Typed tables and collections

A Pop Lang table is a statically typed associative collection. Both key and
value types are known. Heterogeneous values require an explicit union or common
interface.

Illustrative Luau-style syntax:

```luau
local scores: {[String]: Int} = {
    alice = 10,
    bruno = 12,
}

local names: {String} = { "Alice", "Bruno" }
names[1] = "Aline"
```

`{T}` denotes an array type and `{[K]: V}` denotes a typed associative table.
Specialized ordered/persistent collections can be ordinary generic library
values without changing literal semantics. Every read and write remains
statically typed.

Tables do not define lexical namespaces, class identity, ordinary method
lookup, module initialization, records, or tuples. Pop Lang does not inherit the
full Lua metamethod system. Operator customization and iteration use explicit
typed protocols.

Indexed array assignment uses the same one-based indexing model as reads. The
assigned value must have the array's element type, and an out-of-bounds write
traps rather than growing the array or falling back to table semantics.

Arrays have fixed length. `Array.create<<T>>(length, initialValue)` constructs a
fully initialized array, `Array.length(array)` queries its length,
`Array.get(array, index)` performs a trapping non-optional read, and
`Array.fill(array, value)` replaces every element. Ordinary `array[index]`
reads remain optional. See ADR 0034.

## Control flow and loops

`while` remains the pre-condition loop. Pop Lang also has the Luau-shaped
body-first form:

```luau
repeat
    value = value + 1
until value == 3
```

The body executes once before each `Boolean` `until` condition. `true` exits
and `false` repeats. The body and condition share one lexical scope, so a body
local can contribute to the condition without escaping after the loop. The
first loop slice has no `break` or `continue`; generalized iteration remains a
separate typed-protocol feature. See ADR 0032.

## Namespaces, using directives, Modules, and Bubbles

Every source file is a Module inside one file-scoped namespace. Packages declare
Bubble dependencies in `bubble.toml`; `using` makes available namespaces
convenient to name. None of these concepts is a runtime table.

Canonical direction:

```luau
namespace Game.Players

using Shared = Studio.Shared
```

Like C#, `using` affects name resolution only. It does not load a file or run
initialization. The locked Bubble graph and Bubble manifests select and load
implementation artifacts. Pop Lang keeps semicolon-free Luau aesthetics.

The one reserved exception is the fixed `Pop` prelude: normal projects can use
`Math`, `Text`, `Io`, `Json`, `List`, and other common standard names without a
`using`. External libraries remain explicit.

Module rules:

- every namespace-scope declaration resolves to `public`, `internal`, or
  `private` visibility; omission defaults to `internal` except that an omitted
  binary-root `main` is `private`;
- `public` declarations enter Bubble reference metadata;
- `internal` declarations are visible across Modules in the same Bubble;
- `private` declarations are visible only in their Module;
- `using` binds namespace visibility and aliases, never runtime values;
- dependencies form an explicit graph;
- cyclic runtime initialization is rejected;
- type-only dependencies do not require runtime initialization;
- visibility is enforced during resolution and compile-time reflection;
- compilation identity is separate from filesystem spelling.

The complete hierarchy is `Item → Module → Bubble → Package → Workspace`.

An omitted function result annotation denotes an empty result pack. It does not
request return-type inference: valued returns require explicit result types, and
parameters always require explicit types.

The complete artifact/load model is defined in
[Bubbles, namespaces, artifacts, and loading](./14-libraries-namespaces-and-loading.md).

## Records, tuples, and multiple results

Structural records have named typed fields and immutable field bindings. Typed
`with` expressions produce updated records; contained collection/reference types
retain their own mutability. Tuples have ordered typed fields. Neither silently
becomes a heap table.

Luau-style multiple returns remain an ergonomic goal. MIR represents their
static type as a tuple or type pack with known elements/tail constraints.
Destructuring and multiple assignment are syntax over that representation.
There is no dynamically typed variadic result.

## Exhaustive tagged-union matching

The initial `match` is a statement whose arms use `when ... then` and must name
every resolved case exactly once:

```luau
match result
when Result.Ok(value) then
    use(value)
when Result.Error(message) then
    report(message)
end
```

The scrutinee is evaluated once. Payload bindings are statically typed and
arm-local; `_` ignores one payload. Version one has no wildcard arm, guard,
nested pattern, or expression-valued match. HIR retains `UnionCaseId`s and MIR
uses a discriminant switch plus typed payload projections, never tag-name
lookup. See ADR 0021.

## Functions, closures, and methods

A function value has a statically known function type and consists of callable
code plus an optional environment. Non-capturing functions can lower to plain
code references. Captured variables become explicit during closure conversion.

Local functions and anonymous `function ... end` expressions use lexical
capture. Read-only captures copy a typed value; a binding written by an
enclosing or nested function uses one shared typed capture cell. Environments
and cells are native managed objects with deterministic capture identity/order,
not tables. See ADR 0019.

A method is not a table lookup. HIR records the receiver and resolved method.
MIR turns the call into direct, virtual, interface, or statically typed indirect
dispatch. There is no dynamic dispatch category.

The first nominal interface surface contains public instance method signatures.
A class explicitly names interfaces after `implements` and must supply exact
accessible receiver methods. Class-to-interface conversion is a checked static
upcast; calls carry a resolved interface method/slot rather than a name. See
ADR 0020.

## Errors and effects

Recoverable failures use typed result values with light propagation syntax.
`panic` represents violated invariants and unwinds for cleanup/diagnostics before
the task/application policy decides termination. IR distinguishes:

- normal returns;
- checked failure represented by typed results;
- runtime traps such as bounds violations or impossible-state assertions;
- panic unwinding and cleanup edges;
- cancellation and suspension;
- typed foreign-function failures.

All exits are explicit enough for LLVM and a VM to implement identically.

Function types, HIR/MIR functions, and calls carry closed effect summaries.
Checked operations name their `TrapKind`; panic calls record whether unwinding
propagates or enters a cleanup block, and cleanup resumes unwinding explicitly.
Expected failure remains a typed result value and is not folded into the panic
mechanism. See ADR 0022.

## Memory management

The initial runtime uses Pop GC: a precise concurrent generational collector
with a moving nursery and mostly non-moving mature heap. User finalizers and weak
references are excluded from version one. Observable reachability, identity,
resource cleanup, and FFI pin/handle behavior remain language/runtime contracts,
not exposed heap-layout details.

MIR represents allocation, roots, barriers, and safe points through abstract
operations. It never embeds a collector-specific object header.

See [Garbage collector architecture](./15-garbage-collector-architecture.md).
