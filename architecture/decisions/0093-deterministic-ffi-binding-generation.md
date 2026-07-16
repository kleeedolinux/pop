# ADR 0093: Deterministic FFI Binding Generation

- Status: accepted
- Date: 2026-07-16
- Extends: ADR 0010, ADR 0017, ADR 0055, ADR 0081, ADR 0082, and
  ADR 0086
- Supersedes: the generator input and `native-bindings` physical-encoding
  portions of ADR 0081, ADR 0082, and ADR 0086
- Amended by: ADR 0094 for the closed schema-2 callback-pair form

## Context

ADR 0081 accepts `pop ffi generate <alias>`, reviewable generated Pop source,
binding metadata, and C shims, but leaves the generator's physical input,
typed policy, limits, target selection, publication transaction, and first
implemented parser unspecified. Choosing those only in command code would
leave an architecture gap and could reintroduce shell execution, source
injection, or host-dependent C interpretation.

Pop Lang should describe its own binding contract in a strongly typed,
Luau-shaped format. It does not need an untyped interchange document or a host
header parser to make fixed C signatures and layouts productive. The first
path therefore consumes one canonical declarative `.popc` descriptor. Header
ingestion can follow only through a separately accepted, versioned, bounded
adapter that produces that same descriptor contract.

## Decision

### Manifest-owned generation plans

Each generator alias is declared for one exact platform target:

```toml
[platform."x86_64-unknown-linux-gnu".ffiGenerators]
Zlib = { nativeLibrary = "Zlib", descriptor = "native/zlib.popc", descriptorSha256 = "<lowercase SHA-256>", outputDirectory = "src/generated/zlib" }
```

The alias and optional `nativeLibrary` use `PascalCase`. `nativeLibrary` must
name a common or selected-platform `[nativeLibraries]` entry; omitting it means
the target's default C/system link environment and omits `Ffi.Link` from the
generated source. Every path is package-relative and normalized. Every input
path component is regular and non-symlinked. The descriptor requires an exact
SHA-256 hash. The output must be below `src/generated/`, cannot overlap the
descriptor, and cannot already contain a different generation.

`pop ffi generate <alias> --manifestPath <bubble.toml> --platformTarget
<triple>` requires all three explicit selections. It reads only the selected
platform's generator entry and never falls back to a host entry. Descriptor,
output, target, tool, include, or flag overrides are not command options; the
manifest is the only build-authority input.

### Canonical declarative `.popc` version 1

`.popc` is a schema-versioned, declarative Pop compile-time descriptor/source
format. It is not an executable Pop Module, a macro, or a compiler-evaluated
function. Its grammar is a strict subset of Luau-shaped Pop declarations:

```luau
@Ffi.Binding(
    schemaVersion = 1,
    platformTarget = "x86_64-unknown-linux-gnu",
    producerName = "clang-abi-export",
    producerVersion = "18.1.8",
    outputNamespace = Native.Zlib.Unsafe,
)
namespace Native.Zlib.Binding

@Ffi.C.Layout(size = 8, alignment = 4)
internal record Pair
    @Ffi.C.Offset(0)
    left: Ffi.C.Int
    @Ffi.C.Offset(4)
    right: Ffi.C.Int
end

@Ffi.Foreign("compress", abi = "C")
@Ffi.Binding.CallPolicy(nonblocking = false)
@Ffi.Binding.ParameterPointer(parameter = destination, retention = Ffi.Binding.Retention.Call)
@Ffi.Binding.ParameterPointer(parameter = source, retention = Ffi.Binding.Retention.Call)
internal function compress(
    destination: Ffi.Pointer<Byte>,
    source: Ffi.ReadOnlyPointer<Byte>,
    length: Ffi.C.Size,
): Ffi.C.Int
end
```

The file contains exactly one `@Ffi.Binding` namespace header followed only by
`internal` fixed-layout record declarations and `internal` bodyless foreign
function declarations. It accepts no `using`, constants, aliases, classes,
unions, tables, executable statements, function bodies, compile-time calls,
interpolation, conditional syntax, arbitrary attributes, source fragments, or
runtime declarations. Documentation comments are not part of canonical schema
1. Whitespace, indentation, commas, member/argument ordering, final newline,
and declaration ordering have one formatter-defined canonical form; a
noncanonical file fails before semantic use.

The header supplies version, exact platform target, bounded producer
provenance, and a qualified output namespace whose final component is exactly
`Unsafe`. The manifest—not the descriptor—owns link authority. Producer text
is provenance only and is never treated as a command, path, flag, or plugin.

Records are ordered by Pop name and state exact nonzero size/alignment. Fields
preserve declaration order and state exact byte offsets. Functions are ordered
by Pop name and state one validated external symbol, closed ABI (`C`, `System`,
or `CUnwind`), exact ordered parameters, exact result, and reviewed blocking
policy. Duplicate native or Pop identities fail closed.

Schema 1's closed direct ABI types are Pop's exact fixed integers and floats, the
accepted `Ffi.C` scalar types, `Byte`, a record declared earlier in the same
descriptor, and one of ADR 0082's four pointer constructors around one direct
nonpointer ABI type. No nested pointer, function pointer, array, handle,
managed type, optional unrelated to a pointer, union, bit field, packed record,
flexible member, vector, variadic pack, or untyped value is accepted. A function
may omit a result; `Void` is not a value type. ADR 0094 adds schema 2 without
changing schema 1: only a fully attached `Ffi.Function<TSignature>` and matching
`Ffi.CallbackContext` parameter pair becomes valid.

Every pointer parameter has exactly one
`@Ffi.Binding.ParameterPointer` naming its resolved parameter token and the
only accepted retention, `Ffi.Binding.Retention.Call`. Every pointer result has
exactly one `@Ffi.Binding.ResultPointer` with explicit
`Ffi.Binding.Ownership.Borrowed` or `Ffi.Binding.Ownership.Owned`. Pointer
mutability and nullability remain explicit in its static pointer constructor.
Missing, extra, duplicate, retained, or mismatched policy fails closed. The
descriptor never infers names, nullability, retention, ownership, encoding,
callback lifetime, threading, or safety from a C symbol.

The in-process `.popc` parser version 1 validates lexical tokens into closed
typed descriptor values. It never enters ordinary Pop name resolution,
compile-time HIR evaluation, a backend, or runtime reflection. Descriptor
identifiers are rendered only after token validation; no string becomes Pop or
C source. The parser invokes no process, shell, network, filesystem discovery,
environment expansion, clock, or random source.

### Outputs and fingerprints

A successful generation publishes exactly one output directory containing:

```text
bindings.pop
bindings.c
native-bindings.popc
```

`bindings.pop` declares the exact output namespace, optional manifest-owned
`@Ffi.Link`, internal `@Ffi.C.Layout` records, and internal bodyless
`@Ffi.Foreign` functions. `@Ffi.Nonblocking` is emitted only for an exact true
call policy. Descriptor-only size, offset, and pointer-policy attributes are
not copied into runtime source; their verified facts remain in metadata.

`bindings.c` is a deterministic translation unit reserved for generated
closed shims. Schema 1 emits the fixed no-shim unit and rejects declarations
requiring a shim. It never copies a header, macro, expression, or user-provided
C fragment.

`native-bindings.popc` is a canonical typed metadata descriptor, not runtime
reflection. Its one `@Ffi.GeneratedBindings` header records schema, generator
and parser versions, alias, selected platform/link alias, producer provenance,
normalized input path/hash, a full SHA-256 ABI fingerprint over the canonical
input, and normalized output paths/sizes/hashes. Its declarations preserve the
complete normalized layout, signature, effect, and pointer-policy facts. It
contains no timestamp, absolute path, environment value, host path, opaque
compiler handle, or executable code.

The input is limited to 4 MiB, nesting to 32, records/functions to 4,096 each,
fields/parameters to 256 each, and identifiers, symbols, producer values, and
targets to explicit schema limits. Integer geometry uses checked arithmetic.
Full lowercase SHA-256 fingerprints, not compact execution keys alone, protect
descriptor and output identity.

A future header adapter requires an accepted ADR and fixed tool registry. It
must execute directly with a closed argument vector, scrubbed environment,
exact executable version/digest, target/sysroot identity, wall-time, memory,
process, stdout/stderr, file-count, and output-size budgets. Raw executable
paths, compiler flags, shell strings, response files, plugins, and ambient
include search remain forbidden. Its only semantic result is canonical `.popc`.

### Failure atomicity and diagnostics

Generation validates and renders every byte before creating publication state.
It writes a sibling temporary directory, closes and hashes every output,
re-loads and verifies `native-bindings.popc` plus its inventory, and atomically
renames the directory into an absent destination. An already present verified
byte-identical generation is a successful no-op. A different existing output
fails with a conflict diagnostic; schema 1 never overwrites or partially
replaces it. Every error removes the private temporary directory and leaves an
existing generation unchanged.

Generator errors are typed. `POP5080` is invalid manifest/selection, `POP5081`
is an unsafe path or hash mismatch, `POP5082` is a malformed or noncanonical
descriptor, `POP5083` is deterministic budget exhaustion, `POP5084` is a
target/policy mismatch, `POP5085` is an unsupported ABI declaration, `POP5086`
is an output conflict, and `POP5087` is publication I/O failure. Human text is
presentation; tests and machine tooling consume the code plus typed reason. No
generator fix downloads tools, changes safety policy, or deletes output.

## Consequences

- A checked-in `.popc` descriptor can reproducibly generate useful low-level
  bindings for fixed libc-style signatures and records.
- Pop Lang owns one readable, typed interop vocabulary from descriptor through
  generated metadata; an untyped parallel schema is unnecessary.
- Generated source remains reviewable ordinary Pop Lang. The path adds no
  reflection, macro enumeration, source mixin, runtime lookup, or dynamic value.
- Schema 1 intentionally rejects headers, callbacks, nested pointers,
  arrays/unions/bit fields, variadics, and shim-requiring declarations rather
  than guessing. ADR 0094's schema 2 accepts only the closed first callback-pair
  attachment and retains every other rejection.
- Regeneration never destroys local edits. A changed generation requires
  explicit removal of the old generated directory after review or a future
  separately accepted transactional replacement protocol.

## Alternatives considered

### Use an untyped interchange document

Rejected because Pop Lang can express the contract with its own typed,
Luau-shaped descriptors and avoid a second public type vocabulary.

### Invoke a C parser with user flags

Rejected because executable selection, include lookup, plugins, response files,
and environment-derived flags are host execution authority rather than ABI
facts. A future fixed adapter needs the complete bounded contract above.

### Accept raw header or shim text inside `.popc`

Rejected because it turns the descriptor into source injection and makes output
review look safer than it is. Closed facts generate source tokens;
unsupported syntax is diagnosed.

### Infer pointer facts from names

Rejected because C spellings do not prove nullability, retention, ownership,
or safety. Typed policy is mandatory and generated declarations remain
`internal` under final `Unsafe`.

### Overwrite an existing generated directory

Rejected for schema 1 because portable multi-file replacement is not atomic.
An immutable publish or byte-identical no-op gives a small failure-atomic path.

## Required conformance tests

- exact manifest target/alias/native-library selection, default-C omission,
  sorting, duplicate/unknown-field rejection, and no host fallback;
- regular non-symlinked package-relative descriptor/output paths, exact hashes,
  traversal/absolute/response/control/shell text rejection;
- canonical `.popc` tokens, attributes, argument order, indentation, final
  newline, schema version, UTF-8, size, nesting, count, geometry, target,
  identity, and declaration ordering rejection;
- deterministic source, metadata, no-shim C, inventory, and ABI fingerprint
  bytes across repeated runs and different absolute checkout roots;
- exact scalar/record/read-only/mutable/optional pointer and `C`/`System`/
  `CUnwind` positives plus missing policy and unsupported nested pointer,
  function pointer, array, union, bit-field, packed, flexible, vector, and
  variadic schema-1 negatives;
- schema-2 exact callback/context indices, signature fingerprint, lifetime,
  callback ABI, thread, serialized/non-reentrant/abort policy, blocking-only,
  generated-source, metadata, and schema-1-regression coverage from ADR 0094;
- no inferred safety facts, source/C injection, shell/process invocation,
  compile-time execution, reflection, runtime lookup, ambient paths, timestamps,
  or environment output;
- absent-destination atomic publication, byte-identical no-op, conflict and
  injected-write-failure preservation, temporary cleanup, and metadata reload;
- generated `.pop` parses and type-checks with exact `Pop.Ffi` metadata, while
  any descriptor or output hash mutation fails before compilation.

## Documents/components affected

Package manifest/parser, unified CLI, `.popc` parser/formatter and metadata,
FFI diagnostics, source discovery, architecture conformance tests, and generated
binding fixtures.
