# ADR 0025: Canonical Native Entry and Process Arguments

- Status: accepted
- Date: 2026-07-11
- Supersedes: ADR 0024 standalone native entry clause

## Context

ADR 0024 temporarily allowed standalone native builds to select any one
unambiguous `() -> Int` function. That shortcut conflicts with the canonical
binary entry contract already defined by the Package/Bubble architecture, makes
an unrelated helper eligible to become the process entry, and cannot deliver
process arguments to Pop Lang programs.

The native bootstrap has progressed far enough to remove that exception. The
entry adapter also needs a deterministic boundary between platform argument
bytes and Pop Lang's valid UTF-8 `String` values.

## Decision

Every binary Bubble, including the standalone one-Module native build path,
resolves exactly one canonical entry item:

```luau
private function main(arguments: Array<String>): Int
end
```

Entry selection is performed from the resolved typed program by `SymbolId`.
The compiler never scans MIR for a convenient signature and never performs
runtime string lookup. A declaration named `main` with the wrong visibility,
parameter type, arity, or result type is rejected as an invalid binary entry.
Other functions may return any statically declared result shape, including no
result; only the operating-system entry adapter requires `Int` as process
status.

The native entry adapter accepts the platform argument vector, omits the
executable path, validates every remaining argument as UTF-8, materializes a
managed `Array<String>` with precise reference metadata, and invokes the
selected Pop Lang entry item. It preserves each valid argument exactly,
including empty strings and non-ASCII text. An argument that is not valid UTF-8
causes a closed runtime trap before user `main` begins; conversion is never
lossy and does not create a dynamically typed boundary.

`pop run <source.pop> -- <arguments>...` forwards the arguments after `--`
without interpreting them as compiler options. A directly executed artifact
receives its arguments through the same adapter. The `Int` returned by `main`
is converted to the platform process status according to the target ABI.

This decision supersedes only ADR 0024's temporary standalone `() -> Int`
entry clause. Its handle ABI, native runtime, foundation-library, and prelude
decisions remain accepted.

## Consequences

- Standalone and Package-discovered binary builds share one entry contract.
- A no-argument helper can no longer become the executable entry accidentally.
- Native argument construction is a typed, GC-visible runtime operation rather
  than ad hoc backend memory.
- Programs do not need to return `Int` from ordinary functions; the requirement
  is confined to the process boundary.
- Platforms whose native argument representation is not valid UTF-8 fail
  deterministically instead of silently replacing data.

## Required conformance tests

- the exact private `main(arguments: Array<String>): Int` declaration is
  selected by its resolved `SymbolId` and executes natively;
- missing `main`, wrong visibility, wrong arity, wrong parameter type, and wrong
  result type are rejected rather than selecting another `() -> Int` helper;
- ordinary functions with no result or non-`Int` results remain valid;
- the executable path is omitted and zero, one, empty, and multiple arguments
  retain their order and exact UTF-8 contents;
- non-ASCII UTF-8 arguments are preserved and invalid UTF-8 traps before user
  code executes;
- the argument array carries a precise managed-reference element map and remains
  valid across allocation safe points;
- `pop run ... -- ...` and direct artifact execution use the same adapter;
- MIR-interpreter and native execution observe the same logical argument array
  when their host adapters receive the same valid UTF-8 inputs.

## Alternatives considered

### Keep accepting any unambiguous `() -> Int` function

Rejected because signature scanning discards resolved program identity, selects
unrelated helpers, and maintains a second binary-entry language contract.

### Use a no-argument `main` and expose ambient process arguments

Rejected because ambient process state is harder to test, hides an effect, and
does not match the accepted explicit entry signature.

### Replace invalid platform bytes

Rejected because lossy conversion changes program input silently. Pop Lang
`String` is valid UTF-8; arbitrary bytes belong in an explicitly typed byte API.

## Documents/components affected

CLI/tooling and code units, runtime and ABI, backend architecture, native LLVM
entry lowering, runtime process adapters, driver entry selection, examples, and
cross-backend conformance tests.
