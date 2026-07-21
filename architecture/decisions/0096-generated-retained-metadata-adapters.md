# ADR 0096: Generated Retained-Metadata Adapters

- Status: accepted
- Date: 2026-07-16
- Extends: ADR 0004, ADR 0023, ADR 0032, ADR 0055, and ADR 0058
- Supersedes: the unspecified first-release `@RetainMetadata` target,
  projection, generated-adapter, and physical-descriptor portions of ADR 0004
  and ADR 0023

## Context

Pop Lang already requires runtime reflection to be absent by default and permits
only an explicit narrow retained projection consumed through a generated typed
adapter. The accepted architecture did not fix the source contract, eligible
type graph, adapter identity and protocol, physical schema format, deterministic
limits, public-artifact behavior, or dead-stripping rule. Implementing any of
those choices privately would leave a first-release architecture gap and could
create a runtime field registry, an untyped metadata value, or two incompatible
schema channels.

The first release needs one small useful retention capability. Typed data codecs
need a closed record/enum/union schema and direct typed construction/access, but
they do not need class internals, method discovery, arbitrary UDA values, or a
process-wide type registry. Pop Lang also already has a typed, Luau-shaped
declarative `.popc` format direction for generated compiler contracts. Retained
adapter schemas should use that format rather than introduce JSON as a second
semantic type vocabulary.

## Decision

### Exact first-release source contract

`@RetainMetadata` is one non-repeatable compiler-defined trusted attribute. The
first-release form has exactly two required named arguments in this order:

```luau
@RetainMetadata(
    use = Metadata.Use.Codec,
    schemaVersion = 1,
)
public record User
    name: String
    age: UInt32
end
```

`use` has the closed compiler-known enum type `Metadata.Use`; its only accepted
first-release value is `Metadata.Use.Codec`. `schemaVersion` is a nonzero
`UInt32` application-schema version. It is distinct from the `.popc` descriptor
schema and generated-adapter protocol versions. Positional, omitted, reordered,
duplicate, unknown, or nonconstant arguments fail. A user attribute with the
same spelling never acquires this trusted identity.

The trusted attribute is the `Pop.Standard` prelude identity
`attribute:3`, source spelling `@RetainMetadata`. ADR 0058's append-only API
baseline must contain that exact row before implementation is exposed. The
qualified `Metadata.Use.Codec` and `Codec.Schema<T>` identities are ordinary
public `Pop.Standard` identities, not spellings recognized from an untrusted
Bubble.

The legal targets are non-generic namespace-scope `record`, payload-free `enum`,
and tagged `union` declarations. The attribute is rejected on namespaces,
attributes, aliases, constants, fields, cases, classes, interfaces, functions,
methods, parameters, locals, generic declarations, and foreign declarations.
Schema version 1 therefore cannot expose private class state, invoke methods,
discover tests or RPC endpoints, or retain arbitrary attached UDA values.
Those capabilities require separate accepted ADRs and distinct typed adapter
protocols.

One accepted attachment reserves one compiler-originated sibling Item named by
appending `Schema` to the target name. `User` therefore reserves `UserSchema`.
The generated Item is an immutable value of exact type `Codec.Schema<User>`, has
the same namespace, Module ownership, and visibility as `User`, and carries a
stable identity derived from the target `SymbolIdentity`, the exact
`Metadata.Use.Codec` identity, and adapter protocol version 1. A collision with
an existing `UserSchema` is a compile error. There is no string or source-name
override.

### Closed serializable projection

The retained codec projection contains only:

- target kind, exact target identity, declared visibility, and application
  schema version;
- declaration-ordered record fields with zero-based ordinals, exact source
  labels, and closed projected types;
- declaration-ordered enum cases with their accepted stable discriminants and
  exact source labels;
- declaration-ordered tagged-union cases, exact source labels, and their
  declaration-ordered payload types; and
- exact nested schema identities plus full projection fingerprints.

The accepted leaf types are `Boolean`, the fixed-width integer and float types,
`String`, and `Bytes`. The recursive constructors are `T?`, fixed tuples,
`Array<T>`, `List<T>`, and another retained record, enum, or tagged union using
`Metadata.Use.Codec`. Every nominal type reachable from a projection must be
visible wherever the generated schema is visible and must carry its own exact
compatible retention request. Recursive nominal schema cycles, tables, classes,
interfaces, functions, tasks, compiler handles, raw or FFI pointers, native
resources, mutable object identity, and all other types are rejected in schema
1. Missing proof never becomes an opaque or dynamically typed field.

Source field/case labels are serialized data labels. A generated decoder may
compare an input label only against the bounded closed label set of its exact
schema and immediately select the corresponding numeric ordinal. A label cannot
resolve another program Item, bypass visibility, or enter a global lookup table.
Schema 1 has no alias, rename, omission, default-value, flattening, untagged-
union, custom-constructor, or arbitrary-UDA projection. Format-specific rules
for duplicate keys, number spelling, depth, and input size remain owned by the
typed codec using the schema.

The projection never includes methods, function bodies, source text, XML
documentation, arbitrary attributes, compiler/type handles, HIR/MIR nodes, GC
maps, object layout, backend objects, absolute paths, environment values, or
runtime object addresses. The trusted request itself is not copied into a
runtime UDA collection.

### Generated typed adapter protocol

`Codec.Schema<T>` is a sealed immutable nominal protocol value. Only the
compiler can construct it from a verified schema-1 descriptor. Its protocol
version 1 payload is exactly:

- the stable generated adapter identity;
- the nonzero application schema version and full projection SHA-256;
- a statically typed encode entry with logical signature
  `function(T, Codec.Writer): Result<(), Codec.Error>`; and
- a statically typed decode entry with logical signature
  `function(Codec.Reader): Result<T, Codec.Error>`.

Each entry has one sealed compiler-originated identity consisting of the stable
generated adapter `SymbolIdentity` plus the closed `Encode` or `Decode` role.
The entries are verified typed functions in generated HIR and ordinary MIR,
but they are not additional namespace Items and cannot be named from source.
Local lowering slots and consumer-local IDs are private remappings of that
identity; numeric arithmetic on the schema Item's local `SymbolId` is never an
artifact or language identity. A source-free consumer reconstructs both typed
entries from the verified adapter identity, role, and canonical `.popc`
projection without duplicating structural schema in JSON metadata.

`Codec.Writer` and `Codec.Reader` are sealed format capabilities exposing only a
closed typed scalar/container event vocabulary. They do not accept compiler
handles, arbitrary program values, or member names as symbol lookup requests.
Generated encode/decode bodies use resolved field/case IDs, direct typed access,
and ordinary typed construction. Their errors and effects are normal typed
function contracts; a backend cannot replace them with reflection or a dynamic
fallback.

`Json.decode(text, UserSchema)` is consequently an ordinary statically resolved
call. `UserSchema` is not a runtime registry entry, and a runtime string cannot
select it. A codec may infer an already visible exact schema from a statically
known `T` only when normal resolution finds that generated adapter identity;
failure is a compile error, not a runtime search.

### Exact codec event vocabulary and sequencing

Adapter protocol 1 uses one closed backend-neutral event tape between generated
adapters and sealed `Codec.Writer`/`Codec.Reader` capabilities. Canonical MIR
has exactly two schema-selected operations, `CodecEncode(adapter, value,
writer)` and `CodecDecode(adapter, reader)`. Their `adapter` operand is the exact
reachable generated Item identity and their value/result types must match that
adapter's verified `Codec.Schema<T>` catalog entry. They cannot accept a type,
field, case, or function name.

The tape vocabulary is closed to:

- `RecordStart(memberCount)`, `Member(ordinal, label)`, and `RecordEnd`;
- `EnumCase(ordinal, label, discriminant)`;
- `UnionStart(ordinal, label, payloadCount)`, `Payload(ordinal)`, and
  `UnionEnd`;
- `TupleStart(elementCount)`, `Element(ordinal)`, and `TupleEnd`;
- `SequenceStart(elementCount)`, `Element(ordinal)`, and `SequenceEnd`;
- `OptionalAbsent` and `OptionalPresent`; and
- separate typed `Boolean`, fixed-width integer, fixed-width float, `String`,
  and `Bytes` scalar events.

Generated encoding emits declaration-order record members, the exact selected
enum/union case, declaration-order union payloads and tuple elements, and
increasing sequence ordinals. Generated decoding accepts exactly that nesting
and ordinal sequence. `Member` and case labels are data labels: decoding
compares one label only with the exact adapter catalog's bounded label at the
same ordinal. It never resolves a program Item or consults a registry. A format
adapter may reorder keyed input only while constructing the tape; it must
canonicalize it to declaration order before `CodecDecode` executes.

Each `CodecDecode` consumes exactly one complete top-level value. A reader may
hold later top-level values for subsequent typed decode calls; those later
values are not trailing payload. Extra events inside the selected top-level
record, union, tuple, sequence, optional, or retained nominal remain malformed
unconsumed nested events.

Both operations return ordinary `Result` values and stop at the first error.
Protocol 1 has the closed `Codec.Error` reasons `MalformedInput`,
`LimitExceeded`, and `CapabilityFailure`. Unexpected end or event kind,
unknown/duplicate/missing member, wrong label/ordinal/discriminant/arity,
trailing payload, invalid scalar representation, and unconsumed nested events
produce `MalformedInput`; accepted depth/input limits produce `LimitExceeded`;
and a sealed reader/writer failure produces `CapabilityFailure`. No malformed
input traps, unwinds, or falls back to dynamic decoding.

Protocol 1 fixes the runtime tape limits at 32 nested container/retained-nominal
levels, 65,536 total events, and 65,535 elements in one sequence or `Bytes`
payload. Readers reject an over-limit count before allocation or recursive
descent. Writers stop before emitting an over-limit tape. Both return
`LimitExceeded` through the declared `Result` failure case; they do not trap or
partially publish an output tape.

`Codec.Error` keeps compiler-known type identity 121. Its exhaustive case
identities are fixed as `MalformedInput` = 0, `LimitExceeded` = 1, and
`CapabilityFailure` = 2. At the PLRI boundary those cases map exactly to status
1, 2, and 3 respectively; status 0 is success and cannot construct an error.
HIR, MIR, reference consumers, and every primary backend use these identities
directly rather than private strings or backend-local reason numbers.

Native ABI 1.19 carries this tape through exactly two PLRI operations:
`CodecWriteEvent` and `CodecReadEvent`. `CodecWriteEvent` receives an opaque
capability handle, one closed `UInt8` event tag, `UInt32` ordinal, bounded static
label pointer plus `UInt64` byte length, `UInt64` auxiliary value, and one
`UInt64` scalar payload. `CodecReadEvent` receives an opaque capability handle,
then writes the actual closed `UInt8` event tag, `UInt32` ordinal, label pointer
and `UInt64` length, `UInt64` auxiliary value, and `UInt64` scalar through
separate fixed-width output slots. Generated code validates one exact expected
tag except at an optional, where it accepts exactly `OptionalAbsent` or
`OptionalPresent` before taking the corresponding static branch. The returned
label is a read-only capability-owned byte borrow valid only until the next
reader event call; it is never a managed object pointer. Generated code compares
that bounded label only with the exact static catalog label selected by the
returned ordinal and checks the exact discriminant, arity, count, and scalar
kind before construction.
Integer and floating events fix the payload's signed kind or IEEE bit
interpretation through the event tag; `Boolean` accepts only zero or one;
`String` and `Bytes` carry managed handles published as precise roots across the
call. The `UInt8` status is exactly `Ok`, `MalformedInput`, `LimitExceeded`, or
`CapabilityFailure`. No variadic payload, untyped union, descriptor pointer,
registry key, or runtime Item name crosses this boundary.

The two MIR operations have exact local effects `Allocates` and `GcSafePoint`:
the writer tape and decoded owned values may allocate. Typed failures remain
`Result` values, so the operations do not intrinsically add `MayTrap` or
`MayUnwind`. Generated entry effects are the union of those local effects and
ordinary effects of direct typed value construction. The interpreter, LLVM,
and future VM must implement this same event contract; the experimental C
backend fails closed.

### Canonical typed `.popc` descriptor

For each Module-private, Bubble-internal, or public-artifact visibility boundary
containing accepted requests, the compiler produces one canonical file named
`retained-adapters.popc` in that boundary's private build/artifact location. A
descriptor contains only entries visible through its boundary. Canonical typed
`.popc` is the **sole retained-adapter descriptor, schema, and generation source
format**. A JSON, TOML, XML, binary, or ad hoc compiler-private retained-adapter
schema is forbidden. This rule does not change ADR 0055: `bubble.manifest` and
ordinary public `reference.metadata` remain bounded canonical JSON control
files. They may inventory or reference a public `.popc` digest but cannot
duplicate its structural projection.

Schema 1 is a non-executable, declarative `.popc` subset. It contains exactly
one `@Metadata.GeneratedAdapters` namespace header followed by internal
descriptor-local record, enum, or union projections carrying only the closed
`@Metadata.CodecSchema`, `@Metadata.Field`, and `@Metadata.Case` facts. For
example:

```luau
@Metadata.GeneratedAdapters(
    schemaVersion = 1,
    adapterProtocolVersion = 1,
    producerName = "pop",
    producerVersion = "0.1.0",
    bubbleIdentity = "Example.Models@1.0.0/Models",
    sourceFingerprint = "<lowercase SHA-256>",
)
namespace Pop.Generated.Metadata

@Metadata.CodecSchema(
    target = Example.Models.User,
    adapter = Example.Models.UserSchema,
    schemaVersion = 1,
    visibility = Metadata.Visibility.Public,
    projectionSha256 = "<lowercase SHA-256>",
    sourceModule = "src/user.pop",
    attachmentStart = 128,
    attachmentEnd = 205,
    targetStart = 206,
    targetEnd = 224,
)
internal record Schema0
    @Metadata.Field(source = Example.Models.User.name, ordinal = 0)
    name: String
    @Metadata.Field(source = Example.Models.User.age, ordinal = 1)
    age: UInt32
end
```

The descriptor-local `Schema0`, `Schema1`, and subsequent names are assigned
after sorting entries by stable target `SymbolIdentity`; they never enter normal
source lookup. Qualified target, adapter, field, and case operands are typed
resolved identities, not strings. Producer and Bubble text is provenance only
and cannot become a command, path outside the Package, dependency, or lookup
authority.

The file accepts no `using`, executable statement, function body, compile-time
call, constant initializer, alias, class, interface, table, arbitrary attribute,
source fragment, documentation comment, interpolation, or conditional syntax.
It is not an ordinary Pop Module, macro output, or source-injection channel. One
formatter fixes UTF-8 tokens, LF endings, indentation, argument/member order,
declaration order, decimal integers, string escaping, and exactly one final
newline. Noncanonical input fails before semantic use.

### Identity, fingerprint, version, provenance, and limits

Each projection fingerprint is lowercase full SHA-256 over the domain separator
`Pop.Metadata.CodecProjection/1` plus a canonical semantic projection token
stream. That stream excludes the `projectionSha256`, producer, Module path, and
source-span provenance arguments. It includes the owning Bubble and target
identities, use identity, all three versions, visibility, kind, application
schema version, declaration ordinals/labels/types, and nested full fingerprints.
The compiler recomputes the value and rejects mismatch; a compact key alone is
never authoritative. The complete file receives its own SHA-256 and byte size
in the ordinary artifact inventory.

The header records descriptor schema version 1, adapter protocol version 1,
compiler producer name/version, exact `BubbleIdentity`, and a source semantic
fingerprint. Each entry records the normalized package-relative Module path,
separate UTF-8 byte spans for the requesting attribute and target header, and
target/adapter identities. Diagnostics retain the original attachment plus
generated Item and descriptor-entry provenance. No timestamp, checkout root,
host path, process, clock, random value, environment value, or backend identity
is admitted.

Schema 1 has deterministic hard limits:

- 4 MiB canonical descriptor bytes;
- 4,096 adapters per Bubble;
- 256 fields or cases and 256 total payload slots per declaration;
- 32 recursive projection levels;
- 65,536 total projection nodes per Bubble;
- 1 MiB total retained UTF-8 label bytes; and
- 1,024 UTF-8 bytes for any identifier, qualified identity, producer value, or
  Module path.

All counts and sizes use checked arithmetic. Limit exhaustion is a deterministic
compile error at the requesting attachment. Unknown descriptor, adapter-
protocol, or projection versions fail closed; no reader guesses, silently
upgrades, or falls back to JSON or compiler-private state.

### Pipeline, visibility, artifacts, and dead stripping

The declaration index reserves each generated adapter Item after resolving the
trusted attachment and collision checks. Typed/compile-time analysis constructs
the closed projection, writes canonical `.popc`, re-loads it through the bounded
descriptor parser, recomputes every fingerprint, and compares every fact against
the normally resolved target before generating adapter HIR. The `.popc` parser
does not run compile-time code or create ordinary source tokens. Generated HIR
is compiler-originated, source-mapped, fully typed, backend-neutral, and verified
before canonical MIR lowering.

Visibility never widens:

- a private target and schema remain in the declaring Module;
- an internal target and schema remain inside the owning Bubble; and
- a public schema is legal only when every reachable nominal type is public and
  belongs to the target Bubble or a direct dependency exposing a compatible
  public schema.

A library `.poplib` containing public schemas inventories the public-only
`retained-adapters.popc`. Module-private and Bubble-internal descriptors are
never published. Canonical JSON `reference.metadata` contains only the public
`Codec.Schema<T>` Item identity, exact static type, descriptor path, byte size,
and full file/entry fingerprints needed by a consumer. Structural fields/cases
and source provenance remain exclusively in `.popc`; they do not enter the JSON
public reference surface. Loading verifies the artifact inventory, descriptor,
public visibility closure, and exact typed identity before consumer resolution.

An attachment causes compile-time schema emission but does not by itself force
runtime retention. Whole-program reachability emits a generated adapter body and
its minimal labels/fingerprints only when that exact schema Item is reachable.
Unused private/internal schemas and unreachable public schema implementations
are dead-stripped together. A `.poplib` descriptor can remain as verified
compile/link input without becoming runtime data. There is no process-wide
registration or Bubble-initialization side effect.

### Runtime and backend contract

Canonical MIR contains ordinary typed generated functions and closed codec
operations. It contains no enumerate-types, get-field-by-name, call-by-name,
dynamic box, descriptor parser, or JSON-schema operation. The runtime may supply
backend-neutral codec event services, but does not parse `.popc`, expose its
records, or accept a type/member name. LLVM, the MIR interpreter, and a future VM
execute the same verified MIR and preserve identical labels, ordinals, errors,
effects, and results. The experimental C backend rejects unsupported generated
codec MIR before output rather than synthesizing reflection or incomplete
adapters.

## Consequences

- The first release has one useful, closed retained-metadata capability rather
  than an open-ended reflection promise.
- Existing concise `UserSchema` examples gain one deterministic typed origin.
- Private class state, methods, arbitrary UDA values, compiler handles, and
  unrestricted discovery remain unavailable.
- Public schema exchange is source-free and reproducible without copying a
  structural schema into JSON reference metadata.
- Runtime cost is reachability-driven; a request that is never consumed does not
  force runtime metadata.
- RPC, command/test discovery, custom field aliases/defaults, recursive schemas,
  generic targets, and additional codec shapes remain explicit later
  architecture work.

## Alternatives considered

### Encode retained schemas as JSON

Rejected. JSON remains suitable for ADR 0055 control files, but a retained
adapter schema is a typed semantic contract. A canonical `.popc` descriptor
keeps resolved types, adapter identities, visibility, provenance, and the
projection vocabulary in one reviewable Pop-shaped format without a parallel
untyped schema.

### Retain every UDA and declaration member

Rejected because it leaks private structure, defeats dead stripping, creates a
general reflection registry, and pressures field access toward dynamic boxes.

### Permit classes, methods, RPC, and test discovery in schema 1

Rejected because mutable invariants, invocation authority, lifecycle, and
discovery each require different typed protocols and security review. Codec data
schemas do not authorize those capabilities.

### Generate adapters only in backend code

Rejected because backends would reconstruct source semantics and could disagree.
The adapter is typed and verified before MIR; backends only execute canonical
MIR.

### Resolve a schema by runtime type or string name

Rejected because it requires a registry and runtime reflection. Generated
schema Items participate in normal static resolution only.

## Required conformance tests

- record, enum, tagged-union, optional, tuple, array, list, nested retained type,
  private, internal, public, same-Bubble, and public dependency positives;
- exact generated `UserSchema: Codec.Schema<User>` identity, visibility,
  collision, source mapping, and same-result repeated-build tests;
- wrong target, generic target, wrong/missing/reordered/duplicate argument, zero
  schema version, unsupported use/type, inaccessible nested type, missing nested
  retention, recursive cycle, and limit negatives;
- declaration-order ordinal/label/type, nested fingerprint, target/use/version/
  visibility hash sensitivity, full SHA-256 recomputation, and tamper rejection;
- canonical `.popc` UTF-8/token/attribute/argument/declaration/indentation/final-
  newline tests plus malformed, unknown-version, oversized, traversal, symlink,
  absolute-path, timestamp, environment, and source-injection negatives;
- permanent rejection of JSON retained-adapter descriptors or duplicated JSON
  structural schemas while preserving ADR 0055 JSON control-file compatibility;
- private/internal exclusion and exact public adapter-only
  `reference.metadata` round trips through a source-free consumer `.poplib`;
- unused-adapter dead stripping, exact reachable-adapter retention, and absence
  of registration/initialization side effects;
- verifier negatives forbidding compiler handles, untyped boxes, dynamic calls,
  name-selected fields, descriptor parsing, or backend reconstruction in HIR/MIR;
- generated codec round trips and malformed-input behavior through the MIR
  interpreter and LLVM with equal values, labels, ordinals, errors, and effects;
  and
- experimental C fail-closed tests when the selected generated codec MIR is
  outside its declared capability set.

## Documents/components affected

UDA/compile-time and runtime architecture, compiler pipeline, type checker,
generated Item identity, `.popc` parser/formatter, HIR/MIR verification, Codec
schema protocol, `.poplib` emission/loading, reference metadata, linker dead
stripping, diagnostics/provenance, conformance tests, and the release roadmap.
