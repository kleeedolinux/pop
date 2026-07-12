# ADR 0024: Bootstrap Native Handle ABI and Rust Foundation Libraries

- Status: accepted
- Date: 2026-07-11
- Supersedes: none
- Superseded in part by: ADR 0025 canonical native entry and process arguments
- Superseded in part by: ADR 0030 typed string output overload

## Context

The first LLVM backend needs a concrete native boundary while the production
moving collector and complete object layout are still staged. The runtime
already has a precise bootstrap collector whose managed references are stable
opaque handles. The repository also needs an implementation home for the two
reserved foundation libraries before Pop source-library bootstrapping is
complete.

## Decision

The bootstrap native ABI represents managed references and root handles as
nonzero `u64` values. Zero is the invalid/null handle result for C-compatible
entry points. The bootstrap runtime exports version identity, object/array
allocation, scalar/reference array load/store, and root retain/release operations through stable `pop_rt_*`
symbols. Failures at this narrow C boundary return the documented failure
sentinel; typed PLRI and Rust adapters retain the full `RuntimeFailure` value.

ADR 0039 later names this representation `BootstrapStableHandles`. ABI 1.x
cannot satisfy the production moving-nursery profile; production root relocation
uses mutable typed publications and native ABI major version 2.

Canonical MIR continues to carry `RuntimeOperation` identities, not symbol
strings. The LLVM backend selects the ABI spelling and lowers managed values to
the target representation only inside its private IR.

The bootstrap native backend lowers verified MIR into backend-private IR and
uses Inkwell to parse, verify, target, and emit real LLVM object files. Textual
LLVM remains an inspectable derivative, not the native artifact implementation.
The driver links emitted objects with the Rust runtime and standard-library
archives through the platform linker.

Named PLRI operations are used for tuples, arrays, fields, record updates,
unions, captures, dispatch, and table allocation. They are not a generic
semantic fallback or runtime string lookup.

The initial Rust implementation crates are `pop-internal` and `pop-standard`.
`pop-internal` depends on runtime contracts and owns trusted adapters;
`pop-standard` depends on `pop-internal` and exposes the first typed,
function-first Math, Text, and Sequence foundation. Neither crate changes Pop
source syntax or introduces dynamic values.

Until complete `.poplib` reference loading is available, the verified
`Pop.Standard` bootstrap metadata publishes the first prelude function identity:
`print(Int) -> ()`. ADR 0030 extends this bootstrap surface with the typed
`print(String) -> ()` overload while preserving the integer identity. Name
lookup selects these identities only at prelude priority; ordinary lexical and
declaration bindings still shadow the overload set. Typed AST, HIR, and MIR
carry the selected stable standard-function identity rather than a source
spelling, and the LLVM backend maps it to the corresponding Rust
`Pop.Standard` output adapter.
Standalone `pop build <source.pop>` and `pop run <source.pop>` accept one
unambiguous `() -> Int` entry, whose return value is only the process status.
ADR 0025 supersedes this temporary entry clause with the canonical private
`main(arguments: Array<String>): Int` contract. The rest of this decision
remains accepted.

## Consequences

- LLVM IR can be assembled and executed for pure functions before complete
  object layout and linker orchestration are available.
- Bootstrap handles are explicit and cannot be confused with raw managed
  pointers.
- The C boundary is intentionally smaller than PLRI; richer failures remain
  available through typed native and interpreter adapters.
- The production moving/concurrent collector may replace the implementation
  behind the same semantic PLRI boundary, subject to ABI version checks.
- Standard-library API expansion must preserve the Internal → Standard
  dependency direction and the function/data-first design.

## Required conformance tests

- bootstrap ABI symbols and version identity are exported and stable;
- a generated LLVM module can link against the Rust static runtime and invoke
  bootstrap allocation successfully;
- allocation and root operations return nonzero typed handles and reject
  invalid roots without host-language panics;
- scalar and managed-reference array slots preserve the precise element map,
  one-based indexing, and write-barrier behavior;
- LLVM output uses named PLRI operations and contains no generic semantic
  fallback operation;
- representative LLVM text is accepted by `llvm-as` and pure entry points run
  through LLVM execution;
- Inkwell verifies and emits an object that the platform linker turns into a
  native executable linked with both Rust foundation archives;
- `pop build` produces that executable and `pop run` executes it, with a Pop
  arithmetic example calling `print` through `Pop.Standard` and an
  allocating class example exercising the linked Rust runtime;
- `pop-standard` APIs remain typed and `pop-internal` never depends on it;
- source `print(Int)` resolves to its trusted prelude identity, rejects wrong
  arity/types, yields no value, and is shadowed by an ordinary nearer binding;
- HIR/MIR dumps and MIR round trips retain `StandardFunctionId` rather than the
  source name or host ABI symbol;
- bootstrap collector, MIR interpreter, and LLVM agree on pure arithmetic and
  handle/event contracts.

## Alternatives considered

### Expose raw managed pointers from bootstrap code

Rejected because the bootstrap collector uses handles and the production
collector may move young objects; raw pointer identity would create an
unnecessary compatibility promise.

### Put the first standard APIs in compiler crates

Rejected because it would invert the library ownership boundary and make
`Pop.Standard` depend on compiler implementation details.

## Documents/components affected

Runtime and ABI, backend architecture, LLVM backend, PLRI, native bootstrap
runtime, architecture conformance tests, `Pop.Internal`, and `Pop.Standard`.
