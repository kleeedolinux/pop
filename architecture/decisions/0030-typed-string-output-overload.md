# ADR 0030: Typed String Output Overload

- Status: accepted
- Date: 2026-07-12
- Supersedes in part: ADR 0024 single bootstrap output binding

## Context

ADR 0024 introduced the first executable `Pop.Standard` bootstrap binding as
`print(Int) -> ()`. That vertical slice proved stable prelude identities,
backend-neutral HIR/MIR calls, and native foundation-library linking, but it
left the language unable to print its immutable UTF-8 `String` values.

Changing `print` to accept a universal value would violate Pop Lang's strong
static typing and require an `Object`, dynamic formatting, runtime reflection,
or another catch-all mechanism. Replacing the integer binding would also break
accepted examples and compatibility. The bootstrap therefore needs a narrow
typed extension that preserves both the existing identity and the native
managed-string boundary.

## Decision

The trusted `Pop.Standard` bootstrap metadata publishes two prelude overloads:

- `StandardFunctionId(0)`: `print(Int) -> ()`;
- `StandardFunctionId(1)`: `print(String) -> ()`.

Prelude overload selection uses the statically proven argument types after
ordinary lexical/declaration lookup has had the opportunity to shadow the
prelude name. The compiler accepts exactly one matching signature. It does not
insert conversions, inspect a runtime type, fall back to string formatting, or
resolve a function from source or ABI spelling.

HIR and MIR retain the selected `StandardFunctionId`. Their verifiers check the
exact parameter/result contract for both identities. The MIR interpreter writes
its typed string value directly. LLVM lowers the string overload to a private
backend-selected `Pop.Standard` adapter that accepts the managed `String`
handle.

The bootstrap native runtime exposes a versioned read-only UTF-8 string-copy
adapter for trusted foundation-library use. The adapter validates that a handle
names a `String`, reports the byte length without using a failure-ambiguous
zero-length sentinel, and copies only into a sufficiently sized caller buffer.
Adding this adapter advances the bootstrap native ABI from version 1.0 to 1.1.
`Pop.Internal` owns the unsafe ABI call and presents a checked Rust adapter to
`Pop.Standard`; user source and MIR cannot call this ABI by name. This narrow
boundary does not expose object layout, reflection, mutation, or arbitrary
managed-memory access.

Additional printable types require separate typed overloads or a future
accepted static formatting protocol. There is no catch-all `print` contract.

## Consequences

- `print("teste")` and other valid UTF-8 strings work without imports.
- Existing `print(Int)` programs retain their source and stable semantic
  identity.
- A nearer declaration named `print` continues to shadow the complete prelude
  overload set.
- Backends remain governed by the same typed MIR identities and signatures.
- Native output needs a small trusted string-copy ABI, but no string layout or
  runtime type-name lookup escapes into generated code.
- Bootstrap native ABI consumers must accept minor version 1 for the added
  read-only adapter.

## Alternatives considered

### Accept every value through a universal parameter

Rejected because Pop Lang has no universal `Object`, `Any`, or `Dynamic` value,
and output must not introduce one indirectly.

### Convert every value to `String` implicitly

Rejected because implicit formatting semantics and a formatting protocol have
not been accepted, and implicit conversion would hide allocation and failure
behavior.

### Replace `print(Int)` with `print(String)`

Rejected because it would regress accepted programs instead of extending the
typed prelude compatibly.

### Let the native backend inspect the managed string layout

Rejected because managed object representation belongs to the runtime and may
change with the production moving collector.

## Required conformance tests

- bootstrap metadata contains exactly the two stable typed output identities
  and rejects duplicate or catch-all signatures;
- `print(Int)` and `print(String)` select their respective identities, while
  wrong arity and unsupported types remain compile-time errors;
- a nearer declaration named `print` shadows the complete prelude overload set;
- HIR/MIR dumps and MIR round trips retain both identities without ABI names;
- the MIR verifier rejects an identity/signature mismatch;
- the MIR interpreter and LLVM/native path both execute ASCII, empty, and
  non-ASCII UTF-8 string output consistently;
- the native string-copy ABI rejects invalid/non-string handles and undersized
  buffers without exposing partial data;
- architecture regressions continue to reject `Any`, `Dynamic`, runtime
  reflection, string-based resolution, and dynamic fallback operations.

## Documents/components affected

Base libraries, runtime and ABI, closed decisions, bootstrap metadata, type
checking, HIR/MIR verification, the MIR interpreter, LLVM lowering,
`Pop.Internal`, `Pop.Standard`, native runtime tests, and cross-backend output
tests.
