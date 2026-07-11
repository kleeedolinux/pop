# Syntax and Nomenclature

## Design character

Pop Lang source should be visually calm. The language inherits Lua/Luau's
strengths: few punctuation marks, readable blocks, little ceremony, and code
that resembles structured pseudocode without becoming vague.

The aesthetic test for new syntax is:

1. Can a Luau programmer read it without stopping?
2. Does it add less punctuation than the semantic value it provides?
3. Is the common case short while the uncommon case stays explicit?
4. Does formatting produce one obvious readable shape?
5. Is the construct distinguishable without editor coloring?

Pop Lang does not mix the visual dialects of JavaScript, Rust, D, C++, and C#.
It borrows C#'s namespace/artifact separation and metadata ideas, not braces,
semicolons, or modifier-heavy declarations.

## Canonical naming rules

There is no lowercase `snake_case` in Pop Lang source. Underscores are reserved
for uppercase constants and intentionally ignored values.

Public identifiers use complete words. Do not truncate `Iterable` to `Iter`,
`Configuration` to `Config`, or `Synchronization` to `Sync` merely to shorten a
name. Widely standardized initialisms/technical forms such as `Json`, `Http`,
`Io`, `Utf8`, `Ffi`, `Gc`, `Guid`, and the language term `Async` are allowed and
follow word casing. Namespace context removes repetition instead of chopping
words.

| Entity | Convention | Examples |
| --- | --- | --- |
| Namespace | `PascalCase` components | `Game.Players`, `Pop.Text` |
| Package/Bubble | `PascalCase` components | `Pop.Standard`, `Studio.Gameplay` |
| Class, record, interface, enum, type alias | `PascalCase` | `Player`, `Request`, `Serializable` |
| Built-in type | `PascalCase` | `String`, `Int`, `UInt32`, `Boolean` |
| User-defined attribute | `PascalCase` | `@Serializable`, `@Route` |
| Enum case | `PascalCase` | `Color.Blue`, `LoadState.Ready` |
| Type parameter | `PascalCase`, usually `T`-prefixed | `T`, `TKey`, `TValue` |
| Function and method | `camelCase` | `loadPlayer`, `calculateScore` |
| Field and property | `camelCase` | `displayName`, `currentScore` |
| Local and parameter | `camelCase` | `playerCount`, `requestId` |
| Module/source filename | `camelCase` | `playerService.pop`, `httpClient.pop` |
| Compile-time/runtime constant | `UPPER_SNAKE_CASE` | `MAX_RETRIES`, `DEFAULT_PORT` |
| Ignored binding | `_` only | `_` |

`snake_case` such as `player_count`, `load_player`, or `serializable_attribute`
is rejected by the style checker. Public and private names follow the same
casing; privacy is semantic, not encoded with an underscore.

### Acronyms

Acronyms behave like words:

- `HttpRequest`, not `HTTPRequest`;
- `parseJson`, not `parseJSON`;
- `userId`, not `userID`;
- `XmlWriter`, not `XMLWriter`.

Established two-letter type-domain names may receive a narrow style exception
only through the language style specification, not project preference.

### Attributes

Attribute type names are always `PascalCase`, including compiler attributes:

```luau
@Serializable(version = 2)
@CompileTime
@Inline
```

Attribute names do not need an `Attribute` suffix. `@Serializable` is preferred
over `@SerializableAttribute`.

Built-in types are not lowercase keyword aliases. Write `String`, `Int`,
`Boolean`, `Float64`, `Byte`, and `Never`. The lowercase `nil` spelling is a
literal/keyword, not a type-naming exception.

## Lexical style

- Blocks end with `end`; braces do not delimit executable blocks.
- Semicolons are neither required nor recommended.
- One statement normally occupies one line.
- Commas separate list/data items and remain allowed after the last multiline
  item.
- Parentheses are used for calls and grouping, not around `if`/`while`
  conditions.
- `local` declares local bindings.
- `function` declares functions and methods.
- Type annotations follow names with `:` as in Luau.
- Method declarations/calls preserve colon ergonomics.
- Keywords are lowercase.
- Types are not distinguished with sigils or punctuation.

## File shape

A source file has one file-scoped namespace, followed by `using` directives,
then declarations:

```luau
namespace Game.Players

private const INITIAL_SCORE = 0

@Serializable(version = 2)
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

`namespace` and `using` are header declarations and do not need a matching
`end`. Records and functions use normal Luau block structure.

The `with` expression creates an updated record while preserving field names and
types. It is the preferred shape for simple data transformation; a class is not
needed merely to attach one operation to a value.

## Declaration style

### XML documentation comments

Structured API documentation uses `---` plus XML:

```luau
--- <summary>Finds a player by identifier.</summary>
--- <param name="id">The player identifier.</param>
--- <returns>The player, or `nil` when absent.</returns>
public function findPlayer(id: PlayerId): Player?
end
```

Documentation precedes attributes/declarations, uses PascalCase symbol/type
names inside checked references, and follows the canonical formatting/tag order
defined in [XML documentation comments](./20-xml-documentation-comments.md).

### Visibility and namespace declarations

Pop Lang does not use `export` lists or an `export` declaration prefix.
Namespace-scope declarations state visibility directly:

```luau
public record Player
end

public function findPlayer(id: PlayerId): Player?
end

internal function loadPlayerCache(): Table<PlayerId, Player>
end

private function validateName(name: String): Result<(), NameError>
end

public const MAX_PLAYERS = 64
```

Every namespace-scope record, union, alias, class, interface, enum, attribute,
function, and constant must use one of:

- `public`: visible to dependent Bubbles and present in reference metadata;
- `internal`: visible to every Module in the same Bubble, absent from
  public reference metadata;
- `private`: visible only inside the current Module/file.

There is no implicit namespace-scope visibility. The compiler offers a quick fix
to select one. `local` remains for block/function-local bindings and functions.

The declaration prefix grammar is deliberately small:

```text
namespaceDeclaration := visibility declaration
visibility           := "public" | "internal" | "private"
```

Documentation and attributes precede that prefix. Visibility is stored on the
declared symbol; it is not a separate list maintained elsewhere.

A namespace itself has no visibility modifier. Its visible surface is the set of
public declarations it contains. `using` never changes or forwards visibility.

Functions live directly in namespaces; no static class, singleton object,
public-symbol table, or module return value is needed to contain them.

Record fields and union/enum cases follow their containing public type contract.
Interface members are public by definition. Rare class fields/methods explicitly
use `public`, `internal`, or `private`; `protected` is excluded from the initial
language to avoid inheritance-centered API design.

### Classes and methods

Classes remain available for meaningful identity or encapsulated mutable
lifecycle and retain the familiar Lua/Luau receiver shape:

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

The class supplies native field layout and method resolution. The syntax does
not imply a table, metatable, or implicit string lookup. Records plus plain
functions remain the default for ordinary data.

### Interfaces

Interfaces contain public instance signatures without redundant member
visibility. A class names nominal implementations explicitly:

```luau
public interface Reader
    function read(count: Int): String
end

public class FileReader implements Reader
    public function FileReader:read(count: Int): String
        return ""
    end
end
```

`implements` is a static nominal contract. It does not enable duck typing,
runtime name lookup, interface fields, or default bodies in version one.

### Records and data

Data literals keep Lua's readable keyed form:

```luau
local request: CreatePlayerRequest = {
    displayName = "Ana",
    startingScore = 10,
}
```

The expected type decides whether a literal constructs a record, table, array,
or other supported aggregate. Ambiguous empty literals require an annotation.

### Functions

Return annotations remain visually light:

```luau
local function clampScore(score: Int, maximum: Int): Int
    return Math.min(score, maximum)
end
```

Generic declarations use Luau's angle form; explicit generic calls use Luau's
double-angle form to avoid ambiguity with comparisons:

```luau
private function first<T>(values: {T}): T?
    return values[1]
end

local name = first<<String>>(names)
```

Local functions and anonymous expressions retain Luau's `function ... end`
shape and may capture lexical values:

```luau
local offset = 3
local addOffset = function(value: Int): Int
    return value + offset
end
```

Captured state is statically typed and converted to a native environment, never
a table.

### Tagged-union matching

The initial exhaustive statement uses ordinary block words rather than arrows:

```luau
match result
when Result.Ok(value) then
    use(value)
when Result.Error(message) then
    report(message)
end
```

Every case appears exactly once. `_` may ignore one case payload; wildcard arms,
guards, and expression-valued matches are reserved for later design.

### Compile-time values

Constants use uppercase names:

```luau
private const DEFAULT_TIMEOUT = 5
public const MAX_CONNECTIONS = 1024
```

Namespace constants require explicit visibility. Ordinary locals use `camelCase`
even when the binding model prevents reassignment; uppercase communicates a
named compile-time/runtime constant, not merely immutability.

## `using` style

`using` imports a namespace for name resolution; it does not execute code or
load a file at runtime:

```luau
using Studio.Shared
using Physics = Studio.Simulation.Physics
```

Wildcard punctuation is unnecessary. Ambiguous simple names are errors and are
resolved with a namespace qualifier or alias. `using static`, project-defined
implicit/global usings, and runtime-computed imports are excluded.

The fixed `Pop` prelude is a language/toolchain contract, not a configurable
global-using feature. It selectively exposes declarations marked by the trusted
standard library's `@Prelude` contract, so common code can write `Json.encode`,
`Io.open`, and `Math.min` without imports while child members remain qualified.
Prelude names have lower resolution priority than locals/current namespace/
explicit aliases; `Pop.Json` remains available for intentional conflicts.

## Formatting rules

The canonical formatter owns whitespace. Initial rules:

- four spaces per indentation level, never tabs in emitted source;
- one blank line between top-level declaration groups;
- no blank line immediately inside a short block;
- multiline argument/data lists use one item per line and a trailing comma;
- lines target 100 columns, with syntax-aware exceptions for unbreakable names;
- binary operators have spaces; unary operators do not;
- no alignment with variable runs of spaces;
- attributes appear one per line when they have arguments or when multiple
  attributes are attached;
- namespace documentation may precede `namespace`; otherwise `namespace` is
  first, followed by a blank line and sorted `using` directives;
- comments explain intent and are not used to draw decorative boxes.

The formatter must be deterministic and idempotent. Style diagnostics should be
fixable automatically wherever the correction cannot change meaning.

## Reserved visual complexity

The following forms require exceptionally strong justification:

- nested generic punctuation deeper than normal type syntax;
- declaration blocks using braces;
- keyword modifier chains;
- sigils for ordinary types or values;
- postfix operators with invisible side effects;
- context-sensitive punctuation that changes meaning after type checking;
- macros that introduce syntax the formatter cannot understand.

The language should feel richer than Luau semantically, not noisier visually.
