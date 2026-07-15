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
- supported foreign ABIs, native object formats, callback transitions, and
  unwind boundaries;
- debug information formats.

A missing capability results in an earlier portable lowering, a runtime fallback,
or a clear diagnostic. HIR/MIR must not test “is this LLVM?”

ADR 0039 separates target feasibility from backend implementation capability.
Selecting `ProductionGenerational` requires both target relocation support and
a backend declaration proving precise roots plus relocating managed-reference
lowering. `BootstrapStableHandles` is a separate profile. A missing production
capability is a pre-emission error, never a silent stable-handle downgrade or
no-op barrier.

## LLVM backend

The LLVM backend performs:

1. target and concrete layout selection;
2. lowering canonical MIR operations to LLVM IR;
3. materializing PLRI calls and GC metadata;
4. applying the target calling convention;
5. verifying generated LLVM IR;
6. running an LLVM optimization pipeline;
7. emitting object code, assembly, or optional textual/bitcode IR.

Optimized canonical MIR already contains ADR 0085 storage plans and verified
lifetime/region frontiers. LLVM may choose registers, frame slots, checked
activation-owned side storage, or region layouts, but it cannot independently
decide that a managed allocation is non-escaping or free it on fewer exits. The
existing non-escaping scalar-array special case is a transition implementation:
its analysis must move to portable MIR before it becomes the general contract.

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

For ADR 0081 calls, LLVM declares one external symbol from the resolved foreign
identity, applies the selected target C/system calling convention and verified
ABI layouts, spills the exact live managed roots, and surrounds the call with
one canonical `EnterForeign`/`LeaveForeign` pair. The returned transition
identity and root-slot shape remain coupled across every normal and unwind edge;
LLVM reloads every writable root slot after leave before later managed use.
`Ffi.Nonblocking` selects only the closed bounded mode and never removes the
transition. Missing target unwind support rejects `CUnwind` before emission.
The native `main` adapter balances `AttachManagedThread`/
`DetachManagedThread` around argument decoding and Pop invocation; it cannot
enter managed code or allocate an argument array while unbound.
The driver—not LLVM semantic lowering—consumes the typed `NativeLinkPlan` to
link system/framework/object/archive/shared/import-library inputs. LLVM never
parses raw linker flags or reconstructs ownership, callback, or movement facts.

LLVM keeps ABI 1 stable-handle lowering and separately advertises
`RelocatingManagedReferences` for ABI 2. Its writable-root lowering spills exact
live roots, checks the closed safe-point result, reloads new SSA aliases, and
rewrites later instructions, branch arguments, merges, and loop backedges.
Concrete backend capabilities, target capabilities, and the exact native ABI
descriptor are validated together before artifact emission. Emitted-IR
verification and forced-relocation execution tests gate this capability; an ABI
2 call alone is not sufficient.

ADR 0078 selects the first equivalent lowering: ABI 2 spills opaque managed
tokens to the exact writable array, calls `pop_rt_gc_safe_point_v2`, reloads new
backend-private SSA aliases, and rewrites all observably later uses including
branch arguments, merges, and loop backedges. ABI 1 lowering remains unchanged.
The presence of this path does not enable the relocation capability until old-
SSA-use verification and forced native relocation both pass.

The deterministic native ABI 2 conformance runtime forces every published token
to change and aborts on every stale token. Backend-private writable root cells
carry current tokens through divergent merges and are eligible for LLVM's
ordinary promotion into SSA phis. Lowering rejects a direct old-token operand,
and optimized straight-line, branch/merge, and loop-backedge executions pass
with negative stale-token mutations. The relocation capability remains
disabled until unwind, coroutine, and FFI transition proofs pass. ABI 2 entry
wrappers now query exact descriptor 2.0 before argument decoding or normal
program entry; the stable ABI 1 facade rejects that query and the conformance
runtime accepts it.

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
[ADR 0059](./decisions/0059-experimental-secure-c-transpilation-backend.md).

## Experimental eBPF backend

The experimental eBPF backend is an LLVM-backend mode for producing ELF eBPF
objects from verified MIR under an explicit runtime-contract profile. It
validates before emission, keeps BPF and Inkwell details inside the backend,
and uses LLVM's BPF target rather than a custom instruction emitter in the
first slice.

The initial triples are `bpfel-unknown-none` and `bpfeb-unknown-none`. They
represent ELF, no-OS LLVM BPF targets. Runtime support is selected separately
through profiles such as `linux-ebpf`. PLRI remains a set of abstract runtime
contracts; the `linux-ebpf` profile currently provides only the scalar
contracts needed by the MVP and therefore cannot satisfy requirements for
managed allocation, standard-library adapters, GC roots, closures, interfaces,
coroutines, or similar dynamic representations. Those failures are reported as
missing runtime contracts, not as HIR/MIR language bans.

The MVP supports an explicit XDP program mode, emits a wrapper in an `xdp`
section, and rejects checked arithmetic until trap-preserving lowering exists.
It also has eBPF-specific validation for invalid entry signatures, recursion,
floating point, unproven loop backedges, unsupported MIR operations, and
backend representations that have not been implemented yet. If LLVM BPF is
unavailable, object emission fails with a target diagnostic and no partial
artifact. See [ADR 0071](./decisions/0071-experimental-ebpf-backend.md).

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

At a collecting safe point the VM updates the exact typed managed-reference
register/frame slots from the mutable PLRI root publication before executing the
next bytecode. It advertises relocation only after verification and forced-minor
tests prove this postcondition.

The VM also consumes the same `Elided`, `StaticSlot`, and `ScopedRegion` plans.
It may realize activation storage in typed register/frame arrays, but must end
and bulk-close it at the exact MIR frontiers and preserve outward relocating
roots.

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
- proof-directed storage plans, lifetime frontiers, scoped-region roots, and
  managed fallback;
- errors, cleanup, and stack traces;
- coroutines/tasks;
- FFI boundary behavior where supported;
- `Pop.Standard` public API behavior and `Pop.Internal` intrinsic conformance.

Backend-specific tests supplement this suite but cannot redefine semantics.
