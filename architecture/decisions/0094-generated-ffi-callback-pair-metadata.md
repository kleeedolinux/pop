# ADR 0094: Generated FFI Callback-Pair Metadata

- Status: accepted
- Date: 2026-07-16
- Supersedes: none
- Extends: ADR 0081, ADR 0082, ADR 0086, ADR 0092, and ADR 0093

## Context

ADR 0092 requires trusted generated metadata for the first stable native
callback shape: one statically typed function pointer and its inseparable
opaque context parameter. ADR 0093 deliberately makes `.popc` schema 1 reject
all function pointers and callbacks because it had no closed syntax or
validation rules for that attachment.

Leaving the disagreement to implementation would let ordinary source spelling
authorize callback retention, thread entry, or panic policy. Those facts must
come from the selected, hashed, canonical generator descriptor and must be
checked against the resolved foreign declaration before HIR. They cannot be
attributes copied into ordinary source, runtime strings, reflection data, or a
second type system.

## Decision

### Schema evolution

ADR 0093 `.popc` schema 1 remains byte-compatible and continues to reject every
callback, function-pointer, and `Ffi.CallbackContext` occurrence. This ADR
accepts schema 2. Schema 2 contains all schema-1 declarations and rules and adds
only the closed callback-pair form below. Header adapters, arbitrary function
pointers, callback shims, concurrent/reentrant policy, alternative panic
handling, or another context representation remain unsupported.

A schema-2 descriptor without callback pairs is valid. A schema-1 descriptor
using any schema-2 token fails as an unsupported ABI declaration rather than
silently upgrading itself. Generated metadata records schema and parser version
2 for schema-2 input; schema-1 output retains version 1.

### Canonical callback-pair form

A callback-bearing foreign declaration uses an inline exact function type and
one opaque context parameter:

```luau
@Ffi.Foreign("visit_values", abi = "C")
@Ffi.Binding.CallPolicy(nonblocking = false)
@Ffi.Binding.CallbackPair(
    callbackParameterIndex = 0,
    contextParameterIndex = 1,
    lifetime = Ffi.Binding.CallbackLifetime.CallScoped,
    callbackAbi = Ffi.Binding.CallbackAbi.C,
    signatureFingerprint = "<lowercase SHA-256>",
    thread = Ffi.Binding.CallbackThread.CallingThread,
    concurrency = Ffi.Binding.CallbackConcurrency.Serialized,
    reentrancy = Ffi.Binding.CallbackReentrancy.Forbidden,
    panicPolicy = Ffi.Binding.CallbackPanic.AbortProcess,
)
internal function visitValues(
    callback: Ffi.Function<function(value: Ffi.C.Int, context: Ffi.CallbackContext): Ffi.C.Int>,
    context: Ffi.CallbackContext,
): Ffi.C.Int
end
```

Indices are zero-based declaration-order identities. They are never runtime
lookup keys. The callback index must name exactly one
`Ffi.Function<TSignature>` parameter and the context index must name exactly
one `Ffi.CallbackContext` parameter. `TSignature` is one non-async,
nonvariadic function type with at most one result and exactly one
`Ffi.CallbackContext` parameter. The context position inside `TSignature` need
not equal the foreign pair's context parameter index, but its presence and full
signature are fingerprinted. Every other callback parameter/result type uses
ADR 0093's direct ABI storage recursively; another function type, managed type,
or callback context is rejected.

Every `Ffi.Function<TSignature>` and every foreign `Ffi.CallbackContext`
parameter belongs to exactly one callback-pair attachment. Pair indices cannot
be equal, duplicated, reused by another pair, out of range, or name a result.
The declaration remains bodyless and `internal` under a final `Unsafe`
namespace. Schema 2 does not accept callback results from the foreign
declaration, pointer-to-callback, retained raw context, or a separately usable
function pointer.

The accepted lifetime values are `CallScoped` and `Registered`. The lifetime
determines the first-release thread policy: `CallScoped` requires
`CallingThread`, and `Registered` requires `AttachedThread`. A scoped attached
pair or registered calling-thread pair is rejected. Callback ABI is exactly `C`
or `System`; `CUnwind` is rejected.
The only accepted first-release policy tuple is `Serialized`, `Forbidden`, and
`AbortProcess`. All facts are mandatory even when only one value is currently
accepted, so later expansion cannot reinterpret old metadata.

A callback-bearing foreign declaration must spell
`@Ffi.Binding.CallPolicy(nonblocking = false)`. `true`, a copied
`@Ffi.Nonblocking`, a missing call policy, or disagreement between generated
metadata and resolved source fails before HIR. The resulting runtime transition
is blocking and uses `HandlesOnly`; it can never enter from `BoundedForeign`.

This lifetime-to-thread mapping closes a static-proof gap in ADR 0092.
`Ffi.RegisteredCallback<TSignature>` does not encode the enum value originally
passed to `Ffi.Callback.open`, so permitting both thread cases for the same
registered type would lose the fact through aliases and function boundaries.
The first stable checker therefore accepts only the direct
`Ffi.CallbackThread.AttachedThread` spelling for `open`; registered
`CallingThread` remains a later architecture gap. `Ffi.withCallback` needs no
source thread argument and always produces the call-scoped/calling-thread pair.
Native ABI support for the other lifetime/thread combinations remains
runtime-internal capability, not stable source permission.

### Signature descriptor and fingerprint

The inline function type is the full typed callback signature descriptor.
Parameter names aid review but do not enter ABI identity. The asserted
`signatureFingerprint` is SHA-256 over these exact UTF-8 bytes with LF line
endings and one final LF:

```text
Pop.Ffi.CallbackSignature/1
platformTarget=<selected target>
abi=<C or System>
parameterCount=<decimal count>
parameter[0]=<canonical ABI layout>
...
resultCount=<0 or 1>
result[0]=<canonical ABI layout>
```

The final `result[0]` line is absent when `resultCount` is zero. Canonical
scalar, pointer, record, and context layouts use ADR 0086's target layout
vocabulary. A record is expanded through its declaration-ordered field names,
offsets, recursively canonical field layouts, size, and alignment; a pointer
records its exact constructor and expanded element layout. The context layout
is the literal `Ffi.CallbackContext(pointerWidth=64)`. Lengths, geometry, and
recursion use the existing checked schema budgets. A target whose pointer width
is not exactly 64 rejects schema-2 callback metadata.

The parser recomputes the fingerprint from typed descriptor values and rejects
an unequal assertion. A compact execution key may be derived under ADR 0086,
but generated metadata, public reference metadata, `ForeignFunctionDeclaration`,
HIR, and MIR retain the full lowercase fingerprint and full policy facts.

### Generated outputs and compiler attachment

`bindings.pop` emits the ordinary bodyless foreign declaration with the inline
`Ffi.Function<TSignature>` and `Ffi.CallbackContext` parameter types. It does
not copy `@Ffi.Binding.CallbackPair` into ordinary source and does not expose a
public callback policy UDA. `bindings.c` remains the fixed no-shim unit; schema
2 rejects callback shapes needing a generated C shim.

`native-bindings.popc` preserves each normalized callback attachment, exact
indices, full inline signature, target, lifetime, ABI, full fingerprint, thread,
and fixed policy tuple. Normal Package preflight reloads the bounded typed
metadata, verifies its inventory against `bindings.pop` and the selected
descriptor, and publishes the verified attachments to front-end analysis.

The front end matches an attachment by the selected generated output namespace
and resolved function declaration, then proves the exact parameter count,
indices, `Ffi.Function<TSignature>` and context types, callback signature,
foreign ABI/effects, and fingerprint. A missing, extra, duplicate, stale, or
mismatched attachment is a compile error before HIR. Files outside the
manifest-selected generated directory cannot contribute attachments.

The verified value is stored on `ForeignFunctionDeclaration` and therefore
flows unchanged into HIR, canonical MIR, and public reference metadata. A
consumer revalidates public metadata against its reconstructed exact signature
and target FFI layout catalog before making the pair usable. Internal generated
metadata is not emitted merely because it exists; only a public foreign
declaration's closed attachment can enter consumer metadata. No stage resolves
a function, type, policy, or callback site from a runtime string.

Source pair use is accepted only when both values originate from the same
`Ffi.withCallback` or `Ffi.Callback.withPair` scope and occupy the attachment's
exact callback/context argument indices. The scope lifetime must equal
`CallScoped` or `Registered` respectively. Signature fingerprint, callback ABI,
thread policy, serialized concurrency, forbidden reentrancy, and abort policy
must all match. Either half passed separately, reordered, mixed across scopes,
or passed to an ordinary foreign declaration is rejected.

Opening a callback registration is deliberately ABI-neutral.
`Ffi.Callback.open(callback, Ffi.CallbackThread.AttachedThread)` has no
generated declaration argument, and `RegisteredCallback<TSignature>` does not
forge or erase a hidden ABI choice. It retains only the exact typed callback
environment, source signature, site, and runtime-owned context. The immediate
body of `Ffi.withCallback` and each `Ffi.Callback.withPair` body must instead
resolve all uses of its pair to one unique compatible generated callback
contract. No use is an error, and C/System, fingerprint, layout, lifetime,
thread, or policy disagreement is an ambiguity error before HIR.

The selected pair contract enters HIR and canonical MIR on the lexical pair
operation. A backend emits one fixed thunk per callback-site/contract identity
and supplies that address with the already-open context. Native code may retain
a `Registered` pair according to that contract. A later pair scope may select a
different physical ABI contract for the same still-open registration only when
its source callback signature is exactly compatible; the distinct fixed thunks
share the same opaque context and serialized lifecycle. Selection is always a
compile-time consequence of trusted `.popc` metadata—never a runtime enum,
string lookup, reflection query, or indirect unknown signature.

## Consequences

- Schema 1 remains a small stable direct-ABI format; schema 2 makes the first
  callback shape useful without opening arbitrary function pointers.
- Callback trust originates in one hashed target-selected `.popc` descriptor
  and survives every compiler/artifact boundary as typed data.
- Generated source stays reviewable and statically typed while descriptor-only
  lifetime and runtime-entry policy cannot be forged by source attributes.
- A policy or fingerprint mismatch fails before backend lowering rather than
  becoming runtime reflection, lookup, or a best-effort adapter.

## Alternatives considered

### Amend schema 1 in place

Rejected because existing schema-1 descriptors and rejection tests form a
closed compatibility promise. Schema 2 makes the capability explicit and
allows both parsers to fail closed.

### Copy callback policy attributes into `bindings.pop`

Rejected because ordinary source could forge them and because lifetime/thread/
panic policy is trusted generator metadata, not a user-defined runtime UDA.

### Infer the pair from adjacent parameter types

Rejected because adjacency does not prove retention, thread, ABI, signature
identity, or which context belongs to which callback.

### Store only a compact signature key

Rejected because compact-key collisions require full fingerprint and descriptor
comparison before semantic use.

## Required conformance tests

- schema-1 callback rejection and schema-2 scalar/pointer/record callback
  positives with byte-stable canonical input, source, and generated metadata;
- exact zero-based indices, one callback context in `TSignature`, complete pair
  coverage, duplicate/reused/missing/out-of-range/wrong-type negatives;
- `C`/`System`, call-scoped/calling-thread, and registered/attached-thread
  positives plus `CUnwind`, crossed lifetime/thread, nonblocking, concurrent,
  reentrant, non-abort, malformed, unknown, and omitted-policy negatives;
- exact signature-fingerprint recomputation, target/pointer-width mismatch,
  record-layout mutation, and metadata-tamper rejection before compilation;
- generated source parse/type-check and exact sidecar-to-resolved-declaration
  attachment, including no unselected-directory or ordinary-source forgery;
- same-pair exact-index scoped and registered calls plus reordered, mixed,
  separate, lifetime, ABI, signature, thread, and policy mismatch negatives;
- ABI-neutral registered open, pair-time C/System thunk selection, unused pair,
  incompatible multi-use ambiguity, and one context shared by distinct fixed
  compatible thunks;
- `ForeignFunctionDeclaration`, HIR, MIR, optimization, `.poplib`, and imported
  public reference metadata round trips retaining the complete typed facts;
- architecture regressions forbidding JSON callback schemas, runtime strings,
  reflection, arbitrary function pointers, source policy UDAs, inferred
  lifetime, `Ffi.Nonblocking`, and backend reconstruction.

## Documents/components affected

ADR 0092/0093, compiler pipeline, closed decisions, `.popc` parser/formatter,
generator/preflight, type checking, `ForeignFunctionDeclaration`, HIR, MIR,
reference metadata and `.poplib` verification, diagnostics, and conformance
tests.
