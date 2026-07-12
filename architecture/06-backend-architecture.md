# Backend Architecture

## Backend contract

A backend implements a narrow interface conceptually equivalent to:

```text
Backend
  capabilities() -> BackendCapabilities
  validate(bubble: MirBubble, target: TargetSpec) -> Diagnostics
  compile(bubble: MirBubble, options: CodegenOptions) -> Artifact
```

Supporting services provide layout, symbol mangling, debug information, and
runtime-operation lowering. The exact API depends on the compiler implementation
language, but dependencies must point from a backend toward compiler contracts,
never from MIR toward a concrete backend.

## Capability negotiation

Backends advertise capabilities such as:

- supported integer and floating-point operations;
- exceptions/unwinding;
- tail calls;
- threads and atomics;
- coroutine primitives;
- SIMD;
- precise stack maps and relocating young-GC support;
- shared-library or module loading;
- debug information formats.

A missing capability results in an earlier portable lowering, a runtime fallback,
or a clear diagnostic. HIR/MIR must not test “is this LLVM?”

## LLVM backend

The LLVM backend performs:

1. target and concrete layout selection;
2. lowering canonical MIR operations to LLVM IR;
3. materializing PLRI calls and GC metadata;
4. applying the target calling convention;
5. verifying generated LLVM IR;
6. running an LLVM optimization pipeline;
7. emitting object code, assembly, or optional textual/bitcode IR.

The Rust implementation uses Inkwell only inside the LLVM backend. Verified
canonical MIR first becomes backend-private IR; Inkwell then constructs or
parses that private emission, verifies the LLVM module, applies target data, and
emits the object. No Inkwell or `llvm-sys` value escapes into MIR, the driver,
the runtime interface, or another backend.

The executable entry wrapper is a target boundary, not a second language entry
contract. It maps the platform argument vector through the typed runtime
adapter when the selected entry requests `Array<String>`, calls the canonical
entry `SymbolId`, and maps either its `Int` result or successful no-result
completion to process status. Entry discovery remains a resolved-program/driver
responsibility; the LLVM backend receives the selected ID and never scans names
or signatures to invent an entry.

Native definitions and backend-generated dispatch helpers are mangled with
their owning `BubbleId` plus typed item identity. Two Bubbles may therefore use
the same local raw `SymbolId`, method ID, or helper ID without colliding when a
Package links its library and binary objects. Source names never become linker
identity.

The bootstrap conformance path must assemble emitted textual IR with the target
LLVM toolchain and execute a pure entry point through LLVM's native execution
environment. This proves that the private lowering is real LLVM output without
making LLVM a semantic dependency of MIR.

LLVM opaque pointers, address spaces, intrinsics, and metadata remain confined
to this backend. LLVM IR is disposable output: it can be regenerated from MIR
and is not read by semantic compiler stages.

Compile-time execution never runs through LLVM. This keeps editor analysis,
cross-compilation, cache behavior, and the future VM deterministic and aligned.

## Experimental C backend

The experimental C backend lowers optimized verified canonical MIR to one
deterministic ISO C11 translation unit. It uses exact-width scalar types,
backend-private checked arithmetic helpers, ID-derived symbols, and explicit
control flow. The generated source is shaped for normal C compiler inlining and
optimization, but optimization can never remove an observable Pop Lang trap or
depend on C undefined behavior.

The initial runtime-free capability set covers scalar constants and operations,
direct calls, branches, returns, and the stable typed integer/string output
identities. Literal strings lower to private byte slices and C standard I/O;
their bytes are emitted numerically rather than injected as C text. Managed
allocation, PLRI, other standard calls, aggregate object layouts, dispatch,
closures, panic and unwinding, coroutines, unsafe memory, and FFI are rejected
during backend validation. The backend does not invent a placeholder runtime or
expose a raw pointer fallback.

The bootstrap driver exposes this experiment as `pop transpile <source.pop>
--to c`. Successful output is C source on standard output; failure publishes no
partial artifact. C text is disposable output and is not a stable ABI, cache,
or semantic contract. See
[ADR 0031](./decisions/0031-experimental-secure-c-transpilation-backend.md).

## Future VM backend

The VM backend should lower canonical MIR to typed or register-based bytecode.
It may use a VM-specific low-level IR inside its own directory. That private IR
can choose compact instructions, inline caches, tagged values, and interpreter
dispatch without changing canonical MIR.

Expected VM components:

- bytecode schema and verifier;
- constant and type tables;
- loader with PLRI/version checks;
- interpreter;
- runtime operation adapter;
- stack maps and coroutine frames;
- optional profiling/JIT interface.

The VM is an architectural acceptance test: if implementing it would require
recovering class, closure, error, or lifetime semantics from LLVM-shaped MIR,
the MIR boundary is wrong.

## Reference interpreter

A slow MIR interpreter is recommended before the full LLVM backend. It provides:

- executable semantic tests;
- differential testing against LLVM output;
- a simple place to validate runtime operations;
- proof that MIR is not accidentally LLVM-specific.

It is a compiler verification tool, not necessarily the production VM.

## Backend conformance

All backends run the same observable-language test suite. Tests should cover:

- evaluation order and numeric behavior;
- class layout semantics and dispatch;
- closures and captured mutation;
- Module initialization;
- Bubble identity, linking, and load-context behavior;
- union narrowing, checked casts, and failure behavior;
- typed UDA semantic consequences and metadata-retention boundaries;
- collection semantics;
- errors, cleanup, and stack traces;
- coroutines/tasks;
- FFI boundary behavior where supported;
- `Pop.Standard` public API behavior and `Pop.Internal` intrinsic conformance.

Backend-specific tests supplement this suite but cannot redefine semantics.
