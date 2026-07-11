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

LLVM opaque pointers, address spaces, intrinsics, and metadata remain confined
to this backend. LLVM IR is disposable output: it can be regenerated from MIR
and is not read by semantic compiler stages.

Compile-time execution never runs through LLVM. This keeps editor analysis,
cross-compilation, cache behavior, and the future VM deterministic and aligned.

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
