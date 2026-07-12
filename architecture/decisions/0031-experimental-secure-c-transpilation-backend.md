# ADR 0031: Experimental Secure C Transpilation Backend

- Status: accepted
- Date: 2026-07-12
- Supersedes: none

## Context

Pop Lang needs an additional, inspectable backend experiment that can exercise
canonical MIR without depending on LLVM or the native runtime. Portable C is a
useful distribution and audit format, but treating C as a new semantic source of
truth would risk undefined behavior, unchecked arithmetic, source-name
injection, and backend drift.

The experiment also needs a user-facing invocation. Existing `pop check` dumps
are compiler debug formats and `pop build` emits a native executable, so neither
correctly describes the deliberate production of C source.

## Decision

Add an experimental C backend under `crates/compiler/backends/c`. It consumes
only frozen, verified canonical MIR after portable MIR optimization and emits
deterministic ISO C11 source. C is a disposable backend artifact, never an
upstream IR, cache contract, FFI contract, or definition of Pop Lang semantics.

The bootstrap CLI form is:

```text
pop transpile <source.pop> --to c
```

It writes the complete C translation unit to standard output only after source,
HIR, optimized MIR, and C-backend validation succeed. Diagnostics go to standard
error and failure emits no partial C. The direct source form uses the same
ephemeral single-Module Bubble model as bootstrap inspection and requires a
canonical binary entry. The first slice accepts only a no-argument `main` with
no result or an `Int` result.

The emitted C must be efficient for an optimizing C compiler and secure by
construction:

- use exact-width `<stdint.h>` types and `_Bool`/`stdbool.h` rather than
  platform-width guesses;
- preserve Pop Lang checked integer overflow, division-by-zero, comparison,
  evaluation-order, control-flow, and trap semantics without relying on signed
  overflow, invalid shifts, aliasing violations, or another C undefined
  behavior;
- preserve exact floating-point constant bits without pointer punning;
- run backend validation before rendering and reject unsupported MIR rather
  than inserting unchecked operations or semantic fallbacks;
- derive external and internal C identifiers only from typed Bubble/item/value
  IDs, so source spelling and source text can never inject C tokens;
- emit deterministic declarations and definitions suitable for ordinary C
  compiler optimization, including direct calls and explicit control flow;
- emit no embedded timestamps, host paths, environment values, or ambient
  state.

The runtime-free experimental capability set initially contains fixed-width
integers, `Boolean`, `Float32`, `Float64`, scalar constants and operations,
direct calls, local SSA values, branches, returns, and the two stable typed
`print(Int)`/`print(String)` identities. Literal-backed immutable strings use a
backend-private byte-slice value and typed C standard-I/O adapters; source text
is emitted only as numeric bytes, never as C tokens or an unescaped C string.
This narrow adapter is not PLRI, a managed String layout, dynamic formatting, or
a general standard-library lowering.

The initial set excludes allocated/dynamically produced strings, arrays, tables,
records, unions, classes, closures, indirect/interface dispatch, every other
standard-library call, allocation, GC operations, panic/unwinding, coroutines,
unsafe memory, and FFI. A source program that reaches an excluded operation
receives a deterministic backend capability error. No PLRI stub, unchecked
pointer representation, dynamic lookup, or silent semantic approximation is
permitted.

Expanding that set requires tests against the MIR interpreter and, when runtime
operations are involved, an accepted runtime/PLRI design update. The backend is
experimental: its generated C spelling and helper layout have no compatibility
promise, and it is not a default build backend.

## Consequences

- Developers gain auditable C11 output through one explicit `pop` command.
- Portable MIR optimization is shared with LLVM, the MIR interpreter, and the C
  experiment instead of being recreated from source syntax.
- Checked helpers add some source volume, but optimizing C compilers can inline
  them and eliminate checks proven redundant.
- The initial backend deliberately supports a smaller runtime-free subset and
  diagnoses ordinary managed Pop Lang programs.
- C compiler and platform behavior cannot weaken the accepted Pop Lang numeric
  and control-flow contract.

## Alternatives considered

### Transpile directly from the syntax tree or HIR

Rejected because it would duplicate semantic lowering, bypass canonical MIR,
and make backend disagreement more likely.

### Emit idiomatic unchecked C operators

Rejected because signed overflow and several edge cases would be undefined or
would silently use C semantics instead of Pop Lang trap semantics.

### Bundle provisional runtime stubs

Rejected for the first slice because placeholder managed layouts and no-op GC
operations would create an unsafe second runtime contract.

### Make C the default build backend

Rejected because the capability set is intentionally experimental and smaller
than the accepted language/runtime contract.

## Required conformance tests

- workspace and dependency tests confine the C backend to its backend crate and
  keep portable HIR/MIR crates independent of it;
- deterministic snapshots cover exact-width scalar declarations, mangled
  identifiers, direct calls, control flow, and the entry wrapper;
- generated C compiles as strict C11 and executes the same supported programs
  as the MIR interpreter;
- boundary tests cover every integer width, overflow, division by zero, signed
  minimum division/negation, Boolean operations, and exact float constants;
- generated source contains no Pop source identifier or text capable of C token
  injection and no signed-overflow-dependent unchecked lowering;
- typed integer and literal-string output matches the MIR interpreter without
  embedding Pop source as C text;
- unsupported PLRI, managed allocation, dispatch, unwind, coroutine, unsafe,
  and FFI MIR is rejected before any output is published;
- `pop transpile <source.pop> --to c` is deterministic, writes only C on
  success, and treats missing/unknown `--to` values as usage errors;
- architecture regressions continue to reject dynamic operations and
  backend-specific HIR/MIR.

## Documents/components affected

Compiler pipeline, intermediate representations, backend architecture,
compiler component architecture, implementation roadmap, CLI/tooling contract,
closed design questions, Cargo workspace inventory, architecture tests, C
backend tests, and the `pop` driver.
