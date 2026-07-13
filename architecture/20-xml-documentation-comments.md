# XML Documentation Comments

## Goal

Pop Lang supports structured XML documentation comments with the useful
compiler/tooling behavior of C# XML docs, adapted to Pop Lang's Lua-shaped syntax,
typed errors/effects, restricted reflection, compact APIs, and deterministic
build model.

Documentation is a checked companion to a declaration—not runtime reflection,
not a UDA, and not an unparsed comment blob.

## Comment syntax

Lua/Luau line comments begin with `--`, so Pop documentation comments use three
hyphens:

```luau
--- <summary>
--- Loads a player from the given path.
--- </summary>
--- <param name="path">The save file to read.</param>
--- <returns>The decoded player.</returns>
--- <error type="Io.Error">The file cannot be read or decoded.</error>
public function loadPlayer(path: Io.Path): Result<Player, Io.Error>
end
```

`---` is the canonical documentation delimiter. C#'s `///` is not copied because
it would fight Pop Lang's Lua lexical character. The XML vocabulary/tooling idea
is adopted; the comment token remains native to Pop.

Rules:

- consecutive `---` lines form one XML fragment;
- one optional space after `---` is removed;
- the block attaches to the next declaration;
- compiler attributes may appear between the documentation block and declaration;
- a blank line, ordinary statement, or unrelated token breaks attachment;
- ordinary `--`/block comments never become API documentation;
- a declaration has at most one directly attached documentation block;
- the lossless syntax tree retains raw text/trivia and the semantic model stores
  the parsed documentation tree.

```luau
--- <summary>Represents a serializable player snapshot.</summary>
@Serializable(version = 1)
public record PlayerSave
    playerId: Guid
    displayName: String
end
```

## Documentation targets

Documentation comments can attach to:

- file-scoped namespaces;
- public/internal/private functions and constants;
- records, unions, errors, aliases, classes, interfaces, enums, and attributes;
- fields, union/error/enum cases, interface/class members, and type parameters;
- Module initialization declarations if they become explicit.

Private documentation is useful in source/editor views but is omitted from public
library documentation unless `emitPrivateDocs` is enabled for internal builds.

Namespace documentation is allowed even though a namespace can span files. The
generator merges entries deterministically, rejects conflicting summaries, and
permits additional `<remarks>` sections with source provenance.

## Core C#-compatible concepts

Pop recognizes/adapts these familiar elements:

| Tag | Purpose | Compiler checks |
| --- | --- | --- |
| `<summary>` | Short API description used by hover/completion | one effective summary for each required public declaration |
| `<remarks>` | Longer behavior/design notes | well-formed content |
| `<param name="...">` | Value parameter documentation | name exists, no duplicates, optional completeness rule |
| `<typeparam name="...">` | Type-parameter documentation | type parameter exists, no duplicates |
| `<returns>` | Successful return/result documentation | declaration returns a value |
| `<value>` | Constant/field/case value meaning | valid target kind |
| `<example>` | Usage example | nested code validity when test-enabled |
| `<c>` / `<code>` | Inline/block code | safe XML/text handling |
| `<para>` / `<list>` | Structured prose/lists | valid nesting |
| `<see>` / `<seealso>` | Code/external links | resolved `cref` or safe `href` |
| `<paramref>` / `<typeparamref>` | Inline parameter references | referenced parameter exists |
| `<inheritdoc>` | Reuse an accepted symbol's docs | resolved source, no cycle, compatible signature |

Pop does not center `<exception>` because expected failure uses typed
`Result<T, TError>`. An `<exception>` tag produces a migration warning and quick
fix to `<error>` or `<panic>` unless a future FFI boundary has an explicit mapped
exception contract.

Public-library documentation also records allocation, ownership/copying,
iteration/materialization, blocking/suspension, buffering/backpressure,
dispatch, native/runtime transitions, target availability, and complexity when
relevant. ADR 0032 requires these facts before an API stabilizes; numeric
performance claims require linked reproducible benchmark baselines.

## Pop-specific contract tags

### Typed errors

```luau
--- <error type="Io.Error.Missing">No file exists at the path.</error>
--- <error type="Io.Error.Denied">The process lacks permission.</error>
```

The `type` value resolves statically. On a function returning
`Result<T, TError>`, it must name the exact `TError` or one of that nominal error
declaration's cases. `<returns>` documents only the `Result.Ok` value. Every
reachable public error case requires one non-duplicate `<error>` entry, directly
or through checked inherited documentation; `<error>` on any other return type
is rejected. This is compile-time symbol resolution only; no runtime error
reflection is emitted. See ADR 0052.

### Panic conditions

```luau
--- <panic condition="index &lt; 0">The index is negative.</panic>
```

`<panic>` documents invariant failure, not ordinary recovery. The condition is
documentation text unless a future contract-expression subset is accepted; it is
never evaluated as source or a string mixin.

### Effects

```luau
--- <effect kind="Blocks">Waits for operating-system file I/O.</effect>
--- <effect kind="Allocates">Allocates a buffer proportional to input size.</effect>
```

Recognized kinds are PascalCase effect identities such as `Allocates`, `Blocks`,
`Suspends`, `Unsafe`, `CompileTime`, and `Ffi`. The compiler checks them against
the function's effect summary where one exists. Documentation cannot hide an
effect that the public contract requires.

### Complexity, allocation, and thread safety

```luau
--- <complexity time="O(n log n)" space="O(log n)"/>
--- <allocation>Does not allocate when the destination has capacity.</allocation>
--- <threadSafety>Safe for concurrent readers; writes require synchronization.</threadSafety>
```

The compiler validates shape/target. Library analyzers can compare declared
complexity/allocation tags against benchmark/static metadata where available;
these claims are API contracts even when not mechanically proven.

## Full example

```luau
namespace Saves

--- <summary>
--- Loads and decodes a player snapshot.
--- </summary>
--- <param name="path">The file containing the snapshot.</param>
--- <returns>A decoded <see cref="PlayerSave"/>.</returns>
--- <error type="LoadError.Io">The file cannot be opened or read.</error>
--- <error type="LoadError.Json">The contents are not a valid snapshot.</error>
--- <effect kind="Blocks">Reads from the filesystem.</effect>
--- <allocation>Allocates storage for the decoded snapshot.</allocation>
--- <example>
--- <code language="pop">
--- local result = load(path)
--- </code>
--- </example>
public function load(path: Io.Path): Result<PlayerSave, LoadError>
end
```

## XML model and security

Documentation accepts a safe XML 1.0 fragment subset encoded as UTF-8.

- DTDs, entity declarations, external entities, XInclude, processing
  instructions, and network/file resolution are forbidden.
- Entity expansion is limited to predefined XML entities.
- Parser depth, attribute count, text size, and total documentation size are
  budgeted.
- `href` supports policy-approved schemes (`https` by default); script/data URLs
  are rejected.
- Raw HTML is not accepted wholesale. A small safe formatting set can map to
  `<b>`, `<i>`, `<br/>`, and `<a>`.
- CDATA content is data and never parsed as Pop source unless inside an explicit
  test-enabled `<code>` element handled by the documentation test runner.

Malformed or unsafe documentation cannot alter compiled program semantics.

## Custom tags

Pop preserves well-formed namespace-qualified custom tags for external doc tools:

```xml
<tool:since xmlns:tool="urn:studio:docs">2.0</tool:since>
```

Unknown unqualified tags produce an `ApiDesign`/documentation warning with a
quick fix or configuration action. Custom tags cannot request compiler
capabilities, execute code, retain runtime metadata, or bypass XML security.

## Symbol references

`cref` values resolve through the declaration's namespace, explicit `using`
bindings, and curated prelude. They produce a stable `DocId`, not runtime string
lookup.

```luau
--- See <see cref="Json.Value"/> and
--- <see cref="Json.decode&lt;PlayerSave&gt;"/>.
```

Rules:

- references must resolve uniquely and obey visibility;
- generic angle brackets are XML-escaped;
- overload references include enough signature information to be unique;
- aliases normalize to the target stable documentation ID;
- external `href` links are distinct from code `cref` links;
- renaming tools update resolved references semantically.

## Documentation inheritance

`<inheritdoc/>` is useful for nominal interfaces and deliberately shared
contracts; it is not tied to OOP inheritance.

```luau
--- <inheritdoc cref="Io.Reader.read"/>
public function BufferReader:read(target: Bytes): Result<Int, Io.Error>
end
```

Pop supports direct whole/known-tag inheritance. It does not initially support
arbitrary XPath `path` filters. Inheritance respects visibility, detects cycles,
does not overwrite explicitly supplied tags, and validates parameter/type-
parameter correspondence.

External `<include file="...">` is excluded from version one because hidden file
inputs break dependency tracking and reproducibility. Shared docs use
`<inheritdoc>` or an explicit project documentation input added through a future
tracked design.

## Compiler pipeline

1. Lexer classifies `---` as documentation trivia/token.
2. Lossless parser groups adjacent lines and attaches them through attributes to
   the next declaration syntax node.
3. XML parser builds a safe documentation tree with source offsets.
4. Resolver binds `cref`, `param`, `typeparam`, error/effect identities, and
   inheritance sources.
5. Semantic validation compares documentation with the typed declaration.
6. Documentation diagnostics/quick fixes enter the normal structured diagnostic
   pipeline.
7. Public documentation is emitted separately from runtime code/metadata.

Documentation edits invalidate documentation/hover queries. They do not rebuild
MIR/native code unless an analyzer contract explicitly treats documentation as a
checked public semantic claim.

## Output artifact

`Pop.Standard` and distributable library Bubbles include documentation in `.poplib`:

```text
Library.poplib/
  bubble.manifest
  reference.metadata
  documentation.xml
  targets/
```

`documentation.xml` conceptually contains:

```xml
<doc schemaVersion="1" bubble="Studio.Gameplay">
  <members>
    <member id="function:Studio.Gameplay.Saves.load(Io.Path)">
      <summary>Loads and decodes a player snapshot.</summary>
    </member>
  </members>
</doc>
```

The manifest records `documentationHash` separately from `publicApiHash` and
implementation hashes. Documentation-only changes update editor/doc caches but
do not falsely claim ABI/API-shape changes.

Reference-only Bubble artifacts can ship documentation without implementation code.
Documentation is not loaded into the program runtime unless an explicit resource
embedding feature requests a copy; that is not reflection.

## Tooling

### Language server

- hover shows `<summary>`, parameters, returns, errors, effects, and selected
  remarks;
- completion shows concise summaries;
- signature help shows matching `<param>`/`<typeparam>` docs;
- go-to-definition/rename works for `cref`;
- code actions create/fix documentation;
- documentation diagnostics update incrementally.

### Documentation generator

`pop documentation` consumes reference metadata plus `documentation.xml` and can produce
HTML, searchable JSON, or another documented renderer format. Rendering is
separate from compilation and sanitizes custom/external content again.

### Documentation tests

`<code language="pop" test="true">` examples can be parsed/type-checked in an
isolated documentation test module. Execution, if explicitly enabled, uses the
normal sandbox/capability policy and deterministic inputs. Standard-library
examples are at least compiled in CI.

## Diagnostics and quick fixes

Documentation diagnostics live in a reserved `POP64xx–POP65xx` portion of the
style/API-design range. Initial checks include:

- malformed/unsafe XML;
- orphan documentation block;
- missing/duplicate `<summary>`;
- unknown/missing/duplicate parameter/type-parameter docs;
- `<returns>` on no-result function or missing required returns docs;
- unresolved/inaccessible/ambiguous `cref`;
- documented error not present in the result contract;
- effect tag contradicting effect analysis;
- invalid/inherited documentation cycle;
- unknown unqualified tag;
- missing docs for a public API under enabled `PublicDocs` policy;
- stale code example.

Quick fixes include:

- insert a complete XML documentation skeleton from the signature;
- add/remove/rename `<param>` and `<typeparam>` tags;
- add `<returns>`, `<error>`, `<effect>`, complexity/allocation tags;
- resolve/qualify/update `cref`;
- replace `<exception>` with `<error>`/`<panic>`;
- add `<inheritdoc>` when an exact contract source exists;
- escape invalid XML characters;
- convert an ordinary leading comment block into `---` XML docs.

Malformed documentation is a warning by default so code can still compile. The
standard libraries enable `PublicDocs`, latest warning wave, and warnings-as-
errors. Documentation fixes follow normal safe/review/unsafe and atomic fix-all
rules.

## Standard-library requirements

Every public `Pop.Standard` declaration has:

- a concise complete `<summary>`;
- parameter/type-parameter/return documentation where applicable;
- typed error/panic conditions;
- allocation, blocking/suspension, thread-safety, and complexity contracts where
  relevant;
- at least one compiled example for nontrivial APIs;
- resolved links rather than copied fully qualified prose;
- no OOP-centered explanation for function/data APIs.

`Pop.Internal` documentation additionally records GC/safe-point/unsafe contracts
but is emitted only in toolchain-internal documentation artifacts.

## Formatting

The canonical formatter:

- preserves `---` on every documentation line;
- formats indentation/nesting without changing text/code semantics;
- keeps short `<summary>` on one line when readable;
- preserves `<code>` whitespace;
- orders signature contract tags as summary, type parameters, parameters,
  returns, errors, panic/effects, complexity/allocation/thread safety, remarks,
  examples, see-also;
- never reformats an invalid fragment destructively before offering a preview.

## Architecture boundaries

- Documentation cannot change program semantics, overload resolution, runtime
  reflection, GC layout, or ABI.
- Checked documentation facts come from the same typed semantic model; they do
  not create alternate symbols/types.
- `cref` resolution is compile-time symbol binding, not runtime string lookup.
- `<code>` is not a macro/string mixin.
- documentation output is a tool artifact, not automatically runtime metadata.
- copying C# tags does not import C# exception/OOP semantics.

## C# influence boundary

C# demonstrates the value of triple-line structured XML comments, compiler-
checked parameter and `cref` links, IntelliSense summaries, inheritance, and XML
documentation output. Pop Lang adopts those ideas with `---`, typed errors,
effects/performance contracts, safe XML, deterministic inputs, and no dependency
on runtime reflection.

Primary references:

- [C# XML documentation overview](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/xmldoc/)
- [Recommended C# XML documentation tags](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/xmldoc/recommended-tags)
- [C# documentation comment specification](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/language-specification/documentation-comments)
