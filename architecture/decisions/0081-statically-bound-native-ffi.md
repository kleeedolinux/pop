# ADR 0081: Statically Bound Native FFI

- Status: accepted
- Date: 2026-07-15
- Supersedes: none
- Extends: ADR 0003, ADR 0008, ADR 0022, ADR 0023, ADR 0031, ADR 0032,
  and ADR 0055

## Context

Pop Lang needs productive access to C-compatible native libraries without
weakening its static type, capability, artifact, or garbage-collection
contracts. The existing architecture requires typed FFI declarations to state
ABI, layout, ownership, lifetime, blocking, callback, unwind, and movement
facts, but it does not define their source form, link inputs, artifact metadata,
or generated-binding workflow.

A Crystal-style `lib` block demonstrates the desired brevity, but adopting it
directly would create a second namespace-like container and would conflict with
Pop Lang's Item → Module → Bubble ownership model. Free-form linker flags,
shell command substitution, macro reflection over library methods, and runtime
symbol lookup also conflict with deterministic builds and the absence of
operational reflection.

## Decision

### Source model

Foreign declarations remain ordinary namespace functions. A file-scoped
namespace groups a binding family, and an exact trusted `Ffi.Link` attribute
attaches one or more native-link aliases to that namespace. An exact trusted
`Ffi.Foreign` attribute turns one bodyless function into a statically bound
foreign declaration:

```luau
@Ffi.Link("Pcre")
namespace Example.Pcre.Unsafe

@Ffi.Foreign("pcre2_config_8")
internal function configure(what: Int32, output: Ffi.Pointer<Byte>): Int32
end
```

The attributes are recognized by stable public identities from the explicit
`Pop.Ffi` dependency, never by an unqualified spelling that user code can
shadow. `Ffi.Link` is repeatable on the namespace. `Ffi.Foreign` defaults to
the target C ABI; its closed `abi` argument also accepts `"System"` and
`"CUnwind"`. An omitted `Ffi.Link` means the symbol must resolve from the
target's default C/system
link environment, which keeps ordinary libc bindings concise. The foreign
symbol is a compile/link-time constant and never participates in runtime name
resolution.

A foreign declaration:

- has no executable Pop body, generic parameters, captures, receiver, or
  source variadic pack;
- has exactly one selected ABI and one external symbol;
- uses only types accepted by that ABI's closed mapping;
- contributes an explicit `ForeignFunction` effect and the exact additional
  effects described below;
- lowers to a resolved foreign identity in HIR and MIR, not to an ordinary Pop
  `SymbolId` call whose meaning a backend must reconstruct;
- may be `public` only inside an explicit final `Unsafe` namespace; safe public
  APIs use ordinary wrapper functions and data.

Calling the low-level declaration otherwise uses ordinary call syntax. Pop Lang
does not add a `lib` declaration, a runtime library value, method enumeration,
or reflection API.

### Native link manifest

`bubble.toml` owns native inputs so source text cannot inject linker options.
The accepted deterministic sections are:

```toml
[nativeLibraries]
Pcre = { kind = "system", name = "pcre2-8", discovery = "packageConfiguration", version = "10.42" }
Codec = { kind = "archive", path = "native/libcodec.a", sha256 = "<lowercase SHA-256>" }

[platform."x86_64-unknown-linux-gnu".nativeLibraries]
PlatformCodec = { kind = "shared", path = "native/libplatformCodec.so", sha256 = "<lowercase SHA-256>" }
```

Aliases use `PascalCase`. The closed input kinds are `system`, `framework`,
`object`, `archive`, `shared`, and `importLibrary`. A system entry names one
linker library without accepting arbitrary flags. A framework entry names one
Apple framework. Package-configuration discovery accepts one package name and
an exact or bounded version requirement; the tool invokes the provider
directly, never through a shell, and validates its structured library/search
results against the selected target. Object, archive, shared, and import-library
paths are package-relative regular files with exact SHA-256 hashes. Symlinks,
absolute paths, parent traversal, response files, control characters, and
unrecognized fields fail closed.

Platform sections select exact target triples. Cross compilation never falls
back to host package-configuration or host search paths. The compiler builds a
canonical sorted `NativeLinkPlan`; duplicate aliases, incompatible providers,
missing hashes, and ambiguous symbol providers are errors. Linker arguments are
constructed from typed plan entries. There is no `ldflags` string, shell
backtick, environment expansion, or implicit command execution.

Every `.poplib` records the canonical native requirements, selected target
constraints, ABI fingerprints, provider identities/versions, and hashes. It
does not embed ambient absolute host paths. Consumers merge transitive plans by
identity and reject conflicts before linking. Shared-library deployment remains
an explicit package/install concern; a Pop executable does not search arbitrary
runtime paths.

This ADR covers static binding performed by `pop check`, `pop build`, and
`pop run`. Runtime `dlopen`/`LoadLibrary`, `dlsym`/`GetProcAddress`, and calls
through symbols obtained from runtime strings are not part of the stable FFI.

### Closed ABI type mapping

The `Pop.Ffi` Package owns the explicit unsafe ABI vocabulary and never enters
the prelude. The first stable mapping contains:

- exact Pop integer types `Int8` through `UInt64`, `Float32`, and `Float64` when
  the selected ABI maps them exactly;
- target-specific nominal C scalar types under `Ffi.C`, including `Char`,
  `SignedChar`, `UnsignedChar`, `Short`, `UnsignedShort`, `Int`, `UnsignedInt`,
  `Long`, `UnsignedLong`, `LongLong`, `UnsignedLongLong`, `Size`, and
  `PointerDifference`;
- `Ffi.Pointer<T>`, `Ffi.OptionalPointer<T>`, `Ffi.Function<TSignature>`, and
  `Ffi.OptionalFunction<TSignature>` as opaque unmanaged addresses with no
  implicit integer conversion, arithmetic, dereference, or managed identity;
- `Ffi.Handle<T>` as a generation-checked runtime strong handle distinct from a
  native pointer;
- fixed-layout records carrying the exact trusted `Ffi.C.Layout` attribute.

`Boolean`, `String`, arrays, tables, lists, classes, interfaces, tagged unions,
managed function values, optionals other than an accepted optional pointer, and
unannotated records have no direct foreign representation. Strings cross using
explicit encoding adapters and owned/borrowed buffers. `nil` never becomes a
generic null pointer.

`Ffi.C.Layout` records contain only accepted ABI fields, preserve declaration
order, have no field defaults, and receive target-derived size, alignment, and
offset metadata plus an ABI fingerprint. The compiler verifies those facts
against generated metadata before use by value. Opaque or incomplete C types
are named nominal types used only behind pointers. C bit fields, flexible array
members, packed/unaligned aggregates, C unions, vector extensions, and C
variadics require a generated C-compatible shim with an ordinary closed Pop
signature; the compiler never guesses their layout or applies untyped default
promotions.

### Ownership, movement, and memory access

Raw foreign pointers refer only to unmanaged storage or to a managed value held
by a compiler-verified lexical pin. `Ffi.withPin(value, function(pointer) ...
end)` creates a non-escaping scoped pointer; the checker rejects returning,
storing, capturing, or retaining it, and suspension is forbidden in its scope.
The pin is released on every normal, expected-failure, panic, and cancellation
exit. A foreign declaration cannot claim that a scoped pointer is retained.

Foreign ownership outside one call uses explicit unmanaged allocation/close
APIs or `Ffi.Handle<T>`. Handles carry generation and ownership state and are
updated after relocation. A stale/released handle is detected. Long-lived
native work receives copied unmanaged buffers or handles, never an untracked
managed address.

Pointer load/store, pointer arithmetic, address conversion, and arbitrary
memory copying exist only under `Ffi.Unsafe`. They require exact element types,
alignment, bounds/provenance where available, and explicit checked conversion.
They never manufacture a managed reference or inspect a Pop object layout.

### Calls, effects, callbacks, and unwind

Every foreign call performs the backend-neutral `enterForeign`/`leaveForeign`
transition, publishes precise live roots, and is a GC safe point. Its closed
effect summary always contains `ForeignFunction`, `UnsafeMemory`, and
`GcSafePoint`. It also contains `Blocks` unless the exact trusted
`Ffi.Nonblocking` attribute is present. `Ffi.Nonblocking` is a reviewed promise;
the compiler cannot infer it from a native symbol name.

`enterForeign` receives one mutable precise `RootPublication` plus the closed
call mode `Blocking` or `BoundedNonblocking`. It services the safe point before
native code runs and returns an opaque nonzero `ForeignTransitionId`. The
transition retains the complete logical publication until `leaveForeign`; a
moving runtime writes every relocated root back before either transition
returns. A blocking call converts the publication to runtime-owned strong
handles and places its mutator in `HandlesOnly`, so collection never waits for
the native call. A bounded nonblocking call keeps the publication attached to
the mutator in `BoundedForeign`; it must return within the target's reviewed
bounded-transition contract.

Transition identities are thread-bound, LIFO-nested, and single-use.
`leaveForeign` requires the original identity and exact root-slot shape,
restores `Managed` state before managed code resumes, installs every current
root value, and consumes the identity. A missing managed-thread binding,
wrong-thread identity, mismatched publication, out-of-order leave, duplicate
leave, or transition failure is a runtime invariant panic. Cleanup paths,
including the explicit `CUnwind` boundary, must execute the matching leave
exactly once; an optimizer cannot separate, duplicate, or remove the pair.

Native ABI 1.13 adds distinct `pop_rt_enter_foreign` and
`pop_rt_leave_foreign` entries. The enter entry receives a safe-point identity,
a writable exact root array/count, and the closed `u8` mode (`0` blocking,
`1` bounded nonblocking), and returns a nonzero transition token or zero on
failure. Leave receives that token and the same writable root array/count and
returns a status byte. These new entries do not reinterpret an older ABI 1
safe-point symbol. Native ABI 2 uses the same logical transition with relocating
root writeback; a backend may advertise relocating FFI only after forced-
relocation tests cover both successful return and unwind cleanup.

Native execution must establish a managed-thread binding before the first
managed allocation, root publication, or foreign transition. The generated
program-entry adapter calls backend-neutral `attachManagedThread` before
decoding process arguments or invoking Pop code and calls
`detachManagedThread` after the Pop entry returns. Native scheduler workers use
their already registered dispatch binding instead. Attach returns an opaque
nonzero `ManagedThreadBindingId`; it registers the exact scheduler/mutator pair
in `Managed` state. Detach is thread-bound and single-use, requires no active
foreign or callback transition, changes the mutator to `Detached`, clears the
thread binding, and unregisters it. Duplicate attachment, wrong-thread or stale
identity, active-transition detach, and partial cleanup fail as runtime
invariants.

Native ABI 1.14 adds `pop_rt_attach_managed_thread(i32 scheduler) -> i64`
and `pop_rt_detach_managed_thread(i64 binding) -> i8`. The generated primary
entry uses logical scheduler `1`; zero is invalid. Callback adapters use the
same PLRI identity only when entering from an otherwise unattached native
thread and must detach on every exit. Attaching is not ambient thread-local
discovery: the returned identity is explicit, balanced runtime authority.

Native unwind is forbidden by default. `abi = "CUnwind"` selects the C ABI with
an explicit unwind boundary and adds `MayUnwind`; an incompatible target
rejects it. No native exception or panic crosses into Pop as an untyped payload. A
generated adapter must map an expected native failure to an exact Pop `Result`
or terminate at the declared panic boundary.

Callbacks use `Ffi.Function<TSignature>` values created only by
`Ffi.withCallback` for call-scoped use or `Ffi.Callback.open` for an explicitly
owned registered callback. Entry establishes managed-thread state, scheduler
ownership, root publication, and panic containment before calling a resolved
managed function identity. Call-scoped callbacks cannot escape the foreign
call. Owned callbacks remain rooted until deterministic `close`; their thread,
concurrency, blocking, and reentrancy policy is part of generated metadata.
Closing while native code may still call is an error in the native contract,
not behavior the collector attempts to recover from. A callback panic is
contained and mapped by an exact declared policy; it never unwinds through
foreign frames by default.

### Generated bindings and safe wrappers

`pop ffi generate <alias>` consumes a manifest entry plus explicitly listed
headers or machine-readable ABI descriptions. It runs an approved target-aware
parser directly with bounded resources, records the parser/tool version and all
input hashes, and emits deterministic reviewable `.pop` declarations,
`native-bindings.json`, and required C shims. Generated files are ordinary
source inputs and are never injected as text during compile-time evaluation.

The generator emits low-level declarations as `internal` under a final
`Unsafe` namespace by default. It maps only proven layouts/signatures and emits
diagnostics for unsupported declarations. It never infers ownership,
nullability, encoding, callback lifetime, thread rules, or safety from a C name;
those facts require an exact generator policy file or explicit review. A
generated ABI fingerprint mismatch fails the build.

An ordinary safe wrapper validates ranges and nullability, selects an encoding,
owns cleanup with `defer`, converts native failure into a typed `Result`, and
returns normal Pop data. The FFI effect remains visible in compiler/tooling cost
metadata even when the wrapper removes the caller's unsafe-memory proof
obligation. No attribute can simply relabel an unchecked raw declaration as
safe.

## Consequences

- Common native calls are short and direct after one declarative binding.
- Native dependencies are reproducible, target-specific build inputs rather
  than arbitrary command lines.
- HIR and MIR retain backend-neutral foreign identities, ABI types, effects,
  and transitions; LLVM chooses the physical calling convention and layout.
- The MIR interpreter reports the unavailable native capability unless a test
  installs an exact typed foreign adapter. The experimental C backend rejects
  FFI.
- Generated bindings are auditable source and metadata, not reflection or a
  string mixin.
- Some C surfaces require small generated shims. This is deliberate: secure
  typed interoperability takes priority over pretending every C extension has
  a portable direct representation.

## Alternatives considered

### Add Crystal-style `lib` blocks

Rejected because they duplicate Pop Lang namespaces and introduce a new
semantic container. Ordinary namespaces group foreign functions without
collapsing Module/Bubble ownership.

### Accept raw linker flags and shell command substitution

Rejected because they allow unbounded host-dependent code execution, evade
target validation, and cannot be represented reproducibly in `.poplib`.

### Load libraries and resolve symbols at runtime

Rejected for the stable surface because string-based symbol resolution is an
operational dynamic escape hatch and makes availability a runtime accident.

### Map every Pop value to an opaque C pointer

Rejected because it hides ownership and movement, breaks precise collection,
and encourages native code to depend on private object layout.

### Allow C varargs as an untyped pack

Rejected because default promotions and argument layout would bypass Pop Lang's
exact fixed type packs. Generated fixed-signature shims remain productive and
verifiable.

## Required conformance tests

- namespace `Ffi.Link` attachment and bodyless `Ffi.Foreign` declaration
  parsing, identity resolution, visibility, duplicate/missing ABI, body,
  generic, receiver, and async negatives;
- exact ABI type acceptance plus managed `String`/object/collection, implicit
  null, unannotated/invalid layout, variadic, bit-field, union, and packed
  negatives;
- deterministic manifest parsing and canonical link plans for system,
  framework, object, archive, shared, import-library, package-configuration,
  platform, libc-default, and transitive `.poplib` cases;
- path traversal, symlink, response-file, raw-flag, shell text, host fallback,
  missing/wrong hash, target mismatch, duplicate provider, and malformed-input
  rejection;
- HIR/MIR foreign identity, exact effects, `enterForeign`/`leaveForeign`, root
  publication, unwind action, verifier corruption negatives, and optimization
  preservation;
- LLVM scalar, pointer, fixed-layout, callback, and link/run tests against a
  deterministic native fixture; explicit interpreter/C capability behavior;
- forced relocation across foreign calls, scoped-pin every-exit cleanup,
  pointer-escape/suspension negatives, handle relocation/stale-generation, and
  callback thread/root/panic/close race tests;
- deterministic generator output, bounded malformed-header behavior, ABI hash
  invalidation, unsupported declaration diagnostics, no source injection, no
  reflection, and no inferred safety facts;
- safe-wrapper positive tests for nullability, encoding, ownership cleanup,
  checked ranges, typed native errors, and ordinary Pop return values;
- architecture regression checks forbidding `lib` containers, `ldflags`, shell
  backticks, runtime symbol lookup, dynamic values, raw managed pointers, and
  backend-specific HIR/MIR.

## Documents/components affected

Language/syntax, type system, UDA identities, HIR, MIR, PLRI, LLVM backend, MIR
interpreter capability adapters, GC/root/pin/callback runtime, Package manifest,
lock and `.poplib` encoding, linker, `Pop.Ffi`, generator tooling, diagnostics,
documentation, conformance policy, and roadmap.
