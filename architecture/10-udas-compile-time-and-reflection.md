# UDAs, Compile Time, and Reflection

## Purpose

Pop Lang supports user-defined attributes (UDAs) as typed compile-time metadata.
The idea is influenced by D's ability to attach programmer-defined values to
declarations and inspect them during compilation. Pop Lang intentionally does
not copy D's string mixins, broad string-based member discovery, or unrestricted
compile-time introspection.

The three concepts are separate:

- **UDA:** an immutable typed value attached to a declaration;
- **compile-time evaluation:** deterministic execution of an allowed typed
  function during compilation;
- **reflection/introspection:** restricted queries about symbols or types.

Supporting one does not automatically enable all powers of the others.

## Luau-shaped surface direction

Attributes use the `@` prefix already natural to current Luau syntax. Attribute
declarations and compile-time functions should retain Luau's declaration and
block style.

Illustrative syntax:

```luau
public attribute Serializable(version: UInt32 = 1)
public attribute FieldName(name: String)

@Serializable(version = 2)
private record User
    @FieldName("userName")
    name: String

    age: UInt32
end

@CompileTime
private function serializationVersion<T>(): UInt32
    local value = attribute<<Serializable>>(T)
    return if value == nil then 0 else value.version
end

private const USER_SCHEMA_VERSION = serializationVersion<<User>>()
```

The attribute query uses a resolved type argument and symbol/type handle; it
cannot accept a string name. `Serializable`, `FieldName`, their arguments, the
query result, and `USER_SCHEMA_VERSION` are all statically typed.

## UDA declaration model

An attribute declaration defines:

- a nominal attribute type;
- typed positional/named parameters and defaults;
- valid attachment targets;
- whether it can repeat on one target;
- whether it is visible outside its module;
- whether a permitted data projection may be retained at runtime;
- optional validation implemented by compile-time-safe typed code.

The source contract uses trusted attributes on the attribute declaration rather
than a second declaration block:

```luau
@AttributeUsage(
    targets = { AttributeTarget.Record, AttributeTarget.Field },
    repeatable = false,
)
@AttributeValidator(validateSerializable)
public attribute Serializable(version: UInt32 = 1)
```

`AttributeTarget` is compiler-owned and closed. Omitted `@AttributeUsage`
permits one occurrence on namespace declarations only; it never grants an
unrestricted target set. `@AttributeValidator` names a resolved trusted
`@CompileTime` function, not a string. Its parameters exactly match the
attribute constructor parameters in declaration order and its sole result is
`Boolean`; the compiler supplies normalized canonical arguments and rejects the
attachment when the validator returns `false`. Version one exposes no target
context to validators. Runtime projection remains a separate trusted
`@RetainMetadata` decision. See ADR 0023 and ADR 0096.

The closed declaration targets include nominal `Error` declarations as a
distinct target. `Error` does not alias `Union`, `Class`, or another target even
when their source attachment rules are parallel.

A namespace-targeted attribute precedes the file's `namespace` header, after
any namespace documentation. Attributes written after that header attach to the
following Item; they are never reinterpreted as namespace attributes.

Unless stated otherwise, an attribute:

- may be attached only where its declared target set allows;
- is not inherited by subclasses or overrides;
- is not copied from an interface to its implementation;
- is compile-time-only;
- does not change parsing, visibility, overload resolution, or name binding;
- has no effect merely because its short name matches a compiler attribute.

Compiler-defined attributes live in a reserved namespace or use identities that
cannot be spoofed by a user declaration with the same spelling.

## Attribute value model

UDA arguments are constant expressions whose canonical values may contain:

- booleans, numbers, strings, and enum cases;
- type and symbol references written as resolved syntax, never parsed strings;
- tuples and immutable records;
- immutable arrays with compile-time-known element types;
- other attribute-value types composed from this closed set.

They may not contain:

- runtime object identity or mutable tables;
- closures with captured mutable state;
- raw pointers or foreign handles;
- file descriptors, processes, clocks, random generators, or network resources;
- LLVM values or backend objects;
- a string claimed to be executable Pop Lang source.

Canonicalization gives equal attribute values deterministic equality and hashing
for caching. Attribute values retain origin spans for diagnostics.

## Attachment, ordering, and duplication

The declaration index records UDAs in source order. Semantic consumers normally
query by attribute type, not textual name.

- A non-repeatable attribute appearing twice is an error.
- Repeatable attributes preserve source order when order is semantically exposed.
- Defaults are expanded before canonicalization.
- Attribute arguments can reference only declarations available at that phase.
- An attribute on a generated runtime adapter is compiler-originated and carries
  origin information back to the source attribute.

## Compile-time execution model

### Entry points

Compile-time execution occurs only for explicit reasons:

- a `const` initializer requiring evaluation;
- a UDA argument or default;
- a call required by a type-level/generic decision;
- an explicit compile-time assertion;
- a compiler/Bubble consumer of a UDA;
- an explicitly marked compile-time function invocation.

The compiler may opportunistically fold other pure expressions, but an
optimization cannot change whether a program is accepted or which diagnostics
it produces.

Compile-time functions may use immutable locals, tuples, immutable records,
homogeneous immutable arrays, enum/union values, strings, and typed opaque
symbol/type handles. Their recursive value shapes and handle ownership are
verified before evaluation or caching.

### Interpreter

Compile-time functions execute in a compiler-owned interpreter over typed
compile-time HIR. They are never compiled and run as host-native code and never
sent through LLVM. This prevents host/target differences during cross-
compilation and keeps language-server behavior consistent.

The interpreter defines exact semantics for overflow, floating point, strings,
recursion, and failure. Where target semantics differ, compile-time evaluation
uses Pop Lang language semantics and rejects operations whose result cannot be
known portably.

### Effects and capabilities

The compile-time checker accepts only a restricted effect set:

- immutable local computation;
- bounded temporary allocation;
- calls to other accepted compile-time functions;
- explicit diagnostics;
- permitted UDA and symbol/type queries;
- declared reproducible build inputs, only if a future ADR admits them.

It rejects ambient filesystem access, environment variables, clocks, randomness,
networking, subprocesses, runtime globals, thread scheduling, FFI, inline
assembly, and backend APIs.

An ordinary runtime function is not automatically compile-time-safe merely
because one execution happens to avoid forbidden effects. Eligibility is proved
from its body/effect summary and callees.

### Limits

Every evaluation has configurable but deterministic limits:

- instruction/fuel count;
- recursion and call depth;
- allocation bytes and live values;
- produced collection/string size;
- diagnostic count.

Exhausting a limit produces a diagnostic containing the compile-time call chain
and largest-cost locations. Release builds cannot silently increase limits and
change program meaning without recording the configuration in the build key.

### Incremental caching

The cache key includes:

- compiler and compile-time IR versions;
- function body and transitive callee fingerprints;
- canonical arguments;
- queried symbol/type/attribute fingerprints;
- admitted explicit build inputs;
- semantic options affecting evaluation.

Results are deterministic constants or structured diagnostics. Mutable compiler
objects are never cached as language values.

An evaluation dependency record includes functions, types, symbols, attributes,
canonical arguments, and the complete origin/call chain. Cycles are rejected
before execution. Budgets cover fuel, call depth, allocation, live values,
produced collection/string size, and diagnostic count.

## Restricted compile-time introspection

The default query surface is intentionally smaller than D's broad traits model:

- obtain UDAs from a resolved symbol handle;
- test for a specific attribute type/value;
- inspect the declared target kind and public type signature;
- compare stable type/symbol identity inside one compilation;
- reference a member through normal resolved syntax.

The initial API does **not** provide `allMembers(T)`, `getMember(T, String)`, a
global list of types, function-body AST access, or source locations as forgeable
names. Visibility and module boundaries apply exactly as they do to ordinary
code.

Broader structural enumeration, if needed for serialization or RPC, should be a
specific opt-in capability. A compiler-supported derive consumer can iterate
eligible declared fields as typed handles and emit a statically typed adapter.
It must not expose those handles to runtime code.

The initial source queries are:

```luau
attribute<<Serializable>>(User)
hasAttribute<<Serializable>>(User)
```

The first returns `Serializable?` for a non-repeatable attribute and an
immutable `{Serializable}` for a repeatable attribute; the second returns
`Boolean`. Both operands are resolved compiler handles. There is no by-name
variant or general member/type enumeration.

## UDA consumers

In the first version, user-defined attributes are metadata; they do not rewrite
declarations. Meaning can be supplied by:

1. generic/compile-time library code that queries an attribute and specializes
   an already declared typed function;
2. compiler-supported derive adapters with a documented typed contract;
3. build tooling reading a stable public metadata artifact, if explicitly
   enabled.

A consumer may return constants, diagnostics, metadata projections, or
instances of predefined typed adapter protocols. It may not return source text,
tokens, an untyped AST, arbitrary using directives/Bubble references, or
arbitrary declarations.

ADR 0096 closes the first compiler-generated instance. The exact trusted
attachment

```luau
@RetainMetadata(
    use = Metadata.Use.Codec,
    schemaVersion = 1,
)
public record User
    name: String
end
```

is non-repeatable and legal only on a non-generic namespace-scope record, enum,
or tagged union. It reserves the sibling `UserSchema: Codec.Schema<User>` with
the target's exact Module, namespace, and visibility. Schema 1 rejects classes,
interfaces, functions, members, generic targets, arbitrary UDA retention, and
all uses except `Metadata.Use.Codec`. The generated Item is one instance of a
sealed compiler-defined protocol, not permission for arbitrary declaration
generation.

If Pop Lang later needs user-defined declaration generation, it should use a
hygienic typed builder with constrained output and explicit provenance. That is
a separate feature and requires a new ADR; it is not implied by UDAs.

## No string mixins

Pop Lang provides no equivalent of compiling a string as source. Specifically,
there is no compile-time `eval`, `mixin(sourceString)`, token concatenation,
string-to-symbol lookup, or parser API available to compile-time programs.

This remains true even when a string is a compile-time constant. Strings are
data. They may become user-facing names in serialized output or diagnostics, but
they never acquire lexical scope, visibility, or code-generation authority.

## Runtime reflection boundary

The core runtime reflection API is empty. Programs cannot enumerate all loaded
types, fetch arbitrary fields, invoke methods by name, or bypass private access.

Opt-in retained metadata is a generated data projection, not access to compiler
reflection. The first release has only ADR 0096's closed codec projection. It
contains declaration-ordered record fields or enum/union cases, exact data
labels, closed projected types, application schema version, and full
fingerprints. Its leaf and recursive type vocabulary is closed; every nested
nominal type must be visibly retained for the same use. Classes, methods,
compiler handles, arbitrary attributes, runtime object identity, recursive
schema cycles, and unsupported types are rejected rather than boxed.

Each visibility boundary's canonical typed `retained-adapters.popc` is the sole
descriptor, schema, and generation source format. The compiler re-loads and
verifies it before creating typed HIR for
`UserSchema: Codec.Schema<User>`. Ordinary `.poplib`
manifest/reference control files remain ADR 0055 canonical JSON, but JSON may
only reference the `.popc` path and full digest; it cannot carry a duplicate
structural retained schema.

The generated `Codec.Schema<T>` has exact typed encode and decode entries using
resolved member/case IDs and sealed codec reader/writer capabilities. It does
not provide an untyped `T` field iterator. Input data labels are compared only
against that adapter's bounded closed label set and select fixed ordinals; they
cannot resolve a program symbol or another schema.

This boundary gives Pop Lang:

- dead-strippable metadata;
- stable native and VM behavior;
- no dynamic value representation requirement;
- no automatic privacy leak;
- clearer security review for each reflection-like capability.

Retention preserves normal visibility. A generated schema has the same
visibility as its target, and a public schema requires a completely public
reachable nominal graph. The request emits compile-time `.popc` facts but does
not force runtime data. Only a reachable exact schema Item retains its generated
body and minimal labels/fingerprints; unused adapter implementations are
dead-stripped without registration or Bubble initialization.

## Diagnostics and provenance

Diagnostics produced while evaluating an attribute show:

1. the error at the failing compile-time expression;
2. the compile-time call chain;
3. the UDA attachment or constant that requested evaluation;
4. generated-adapter provenance, when applicable.

Generated adapters retain source mappings to both their consumer definition and
the UDA attachment that caused specialization.

## Testing requirements

The conformance suite covers:

- UDA typing, defaults, repetition, order, visibility, and target validation;
- deterministic evaluation across compiler processes and targets;
- cycle and resource-limit diagnostics;
- incremental invalidation when transitive inputs change;
- rejection of I/O, FFI, backend access, source parsing, and string injection;
- proof that compile-time handles cannot escape to runtime MIR;
- proof that private and unretained metadata cannot be observed at runtime;
- canonical `.popc` bytes, full fingerprint recomputation, deterministic limit,
  malformed/tamper, and permanent no-JSON-schema tests under ADR 0096;
- generated adapter visibility, source-free public artifact consumption, and
  unused-adapter dead-stripping tests; and
- behavior equivalence between the MIR interpreter, LLVM, and a future VM for
  generated adapters.

## Reference boundary

The D UDA model is useful evidence that programmer-defined typed metadata can be
attached to declarations and queried during compilation. Pop Lang deliberately
does not adopt D's string-based member lookup or any string-mixin mechanism.
See the [D UDA chapter](https://dlang.org/book/uda.html) as a design reference,
not a normative specification.
