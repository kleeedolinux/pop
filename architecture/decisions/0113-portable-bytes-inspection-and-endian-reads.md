# ADR 0113: Portable Bytes Inspection and Endian Reads

- Status: accepted
- Date: 2026-07-26
- Depends on: ADRs 0032, 0040, 0051, 0097, and 0110
- Supersedes: none

## Context

The first `Pop.Bytes` surface provides immutable owned storage, direct borrowed
views, checked slicing, optional byte access, and explicit materialization.
Parsers, binary formats, network protocols, and user libraries also need common
comparison, search, and fixed-width integer reads. Reimplementing those loops in
each Package would duplicate bounds policy and make byte-order mistakes likely.

Reusable construction, writes, and text/binary codecs require additional
buffer, sizing, error, and allocation contracts. They cannot be inferred from
the existing immutable-view implementation. The useful inspection subset does
not need those contracts, a native adapter, a compiler-known operation, or a
dynamic byte container.

## Decision

`Pop.Bytes` adds the following ordinary portable functions over immutable
`Bytes.View` values:

```luau
public function equals(left: Bytes.View, right: Bytes.View): Boolean
public function compare(left: Bytes.View, right: Bytes.View): Int
public function startsWith(value: Bytes.View, prefix: Bytes.View): Boolean
public function endsWith(value: Bytes.View, suffix: Bytes.View): Boolean
public function contains(value: Bytes.View, byte: Byte): Boolean
public function indexOf(value: Bytes.View, byte: Byte, start: Int): Int?

public function readUInt16BigEndian(value: Bytes.View, start: Int): UInt16?
public function readUInt16LittleEndian(value: Bytes.View, start: Int): UInt16?
public function readUInt32BigEndian(value: Bytes.View, start: Int): UInt32?
public function readUInt32LittleEndian(value: Bytes.View, start: Int): UInt32?
public function readUInt64BigEndian(value: Bytes.View, start: Int): UInt64?
public function readUInt64LittleEndian(value: Bytes.View, start: Int): UInt64?
```

All indexes are one-based. `indexOf` returns the first matching index at or
after `start`. It returns `nil` when `start` is less than one, greater than the
view length, or no byte matches. `contains` is the concise whole-view form and
is equivalent to testing whether `indexOf(value, byte, 1)` is present.

`equals` performs value equality. `compare` performs unsigned-byte
lexicographic ordering and returns exactly `-1`, `0`, or `1`. A proper prefix
sorts before the longer value. `startsWith` and `endsWith` return true for an
empty prefix or suffix, including when the inspected value is empty.

Each fixed-width read consumes exactly 2, 4, or 8 consecutive bytes starting at
`start`. Big-endian reads treat the first byte as most significant;
little-endian reads treat it as least significant. A read returns `nil` when
`start` is less than one or the complete width is not available. It never
partially succeeds, pads input, wraps an index, or raises a bounds trap.
Every possible bit pattern is representable by the corresponding unsigned
result type.

The names spell out `BigEndian` and `LittleEndian` because an ordinary public
byte-order value type is not yet part of the append-only Standard API-baseline
identity model. This decision does not establish abbreviated aliases or prevent
a later accepted typed byte-order API where it reduces duplication without
runtime dispatch.

## Optimization and portability

The implementations live in the conventionally discovered
`standard/pop/src/bytes.pop` Module and use only `Bytes.length`, `Bytes.get`,
typed optional propagation, comparisons, and checked fixed-width arithmetic.
They lower through ordinary typed HIR and canonical MIR. The compiler, runtime,
interpreter, LLVM backend, and native adapter inventory do not recognize the
new source names.

`equals`, `compare`, `startsWith`, and `endsWith` stop at the first decisive
byte. `contains` and `indexOf` stop at the first match. They take O(n) time in
the bytes inspected and O(1) storage. Fixed-width reads take O(1) time and O(1)
storage. None of the functions allocates, copies byte storage, mutates,
suspends, performs ambient I/O, crosses a native boundary, or uses interface or
dynamic dispatch.

This contract stabilizes deterministic operation counts and allocation
absence, not target-specific timing. A backend may vectorize or inline the
ordinary MIR only when it preserves the exact early-exit, optional, and checked
numeric behavior.

## Consequences

- Binary parsers and protocol libraries gain one checked, portable endian
  policy over zero-copy views.
- Common byte inspection no longer requires owned materialization or sentinel
  values.
- Immutable view lifetime and lender rules remain unchanged.
- `Bytes.Buffer`, writes, bit operations, hex, base32, and base64 remain
  incomplete Bytes work requiring focused accepted contracts.

## Alternatives considered

### Add compiler-known byte algorithms

Rejected because the accepted behavior is expressible efficiently through
ordinary typed Pop source and canonical MIR. Special name recognition would
duplicate semantics across the compiler and backends without evidence.

### Use one Boolean endianness parameter

Rejected because a Boolean call site does not state which order `true` means.
Separate complete names are explicit until a public nominal byte-order type has
an accepted artifact and API-baseline identity.

### Trap on a short read

Rejected because parser input truncation is an expected data condition.
Optional absence composes with postfix `?` and does not hide integer overflow or
another runtime failure.

### Return signed `Int`

Rejected because every 64-bit byte pattern must remain representable. Returning
`UInt16`, `UInt32`, and `UInt64` preserves the exact bits without a widening
allocation or negative reinterpretation.

## Required conformance tests

- equality and comparison cover empty, equal, mismatch, and proper-prefix
  inputs;
- prefix and suffix checks cover empty patterns, shorter inputs, matches, and
  mismatches;
- search covers first/later matches, invalid starts, the final index, and
  absence;
- every endian width covers zero, maximum, asymmetric byte patterns, non-first
  starts, and insufficient input without a bounds trap;
- public documentation records indexes, optional outcomes, allocation, and
  complexity;
- the API baseline appends the exact prototype signatures without widening the
  prelude;
- public reference metadata contains one ordinary source implementation for
  every function;
- MIR interpreter and LLVM executions agree on values and absence; and
- architecture tests reject a Rust duplicate, bootstrap function, intrinsic,
  native adapter, or compiler/backend recognition of the new names.

## Documents/components affected

Bytes source, checked documentation, Standard API baseline, core catalog,
examples, contributor inventory, foundation tests, interpreter/LLVM
differential tests, architecture tests, and roadmap.
