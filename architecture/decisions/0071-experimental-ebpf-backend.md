# ADR 0071: Experimental eBPF Backend

- Status: accepted
- Date: 2026-07-14
- Supersedes: none

## Context

Pop Lang's canonical MIR is intentionally backend-neutral. A constrained eBPF
experiment can exercise that boundary for a kernel-oriented artifact, but it
must not turn Pop Lang into an implicit runtime inside the Linux kernel or leak
LLVM/BPF details into HIR, MIR, the driver, or target-independent crates.

PLRI is the abstract runtime-interface contract layer. It describes what a
program requires from a selected runtime profile; it is not itself the runtime
implementation and does not make every target provide allocation, GC,
standard-library adapters, or dynamic dispatch.

LLVM already provides a BPF target and ELF emission path. Reusing it for the
first slice gives Pop Lang a real object pipeline without committing to a custom
eBPF instruction emitter or a stable kernel ABI surface.

## Decision

Add an experimental `PopBpf` path inside the LLVM backend. It consumes verified
canonical MIR, derives runtime-contract requirements from MIR, resolves those
requirements against the selected runtime profile, runs dedicated eBPF target
validation, renders backend-private LLVM IR, initializes LLVM's BPF target, and
emits an ELF eBPF object. LLVM and Inkwell values remain private to the LLVM
backend.

The initial target triples are:

- `bpfel-unknown-none` for little-endian eBPF;
- `bpfeb-unknown-none` for big-endian eBPF.

Both are ELF targets with no conventional operating system. They record LLVM
BPF compatibility as a target capability. They do not themselves advertise
shared libraries, threads, unwind, stack maps, SIMD, coroutines, dynamic
loading, or GC relocation support. Runtime semantics are selected separately
through runtime profiles. If the linked LLVM was built without BPF support,
object emission fails before publishing an artifact and reports a backend
target diagnostic.

The initial runtime profile for this path is `linux-ebpf`. It provides only the
minimal contracts needed by scalar code: fixed stack storage, integer
operations, direct calls, and static data. It intentionally does not provide
managed allocation, GC, standard-library adapters, closure environments,
interface dispatch, coroutine scheduling, kernel helpers, maps, or ring
buffers. Programs that require those contracts fail contract resolution before
backend lowering. This is a profile limitation of the current implementation,
not a HIR/MIR rule that Pop strings, classes, collections, closures, or PLRI do
not exist.

The MVP supports an explicit XDP program mode selected by CLI:

```text
pop build <source.pop> \
    --target bpfel-unknown-none \
    --runtime-profile linux-ebpf \
    --bpf-program xdp \
    --emit-object <object.o>
```

The Pop entry point is the ordinary resolved binary entry in bootstrap source
mode. For XDP it must be a scalar function returning `Int` whose MIR runtime
requirements are satisfied by the selected profile; the backend generates an
`xdp` section wrapper named `pop_bpf_xdp` and maps the returned scalar to the
XDP action code. The first example returns numeric `2` (`XDP_PASS`). The MVP
does not expose an XDP context value to source code.

The initial supported subset is deliberately small:

- `Boolean`;
- fixed-width integers and `Int`;
- scalar enum constants;
- scalar constants;
- integer and Boolean operations that the backend can lower without changing
  Pop Lang trap semantics;
- comparisons and Boolean/bitwise operations already represented in MIR;
- explicit branches without unproven loop backedges;
- non-recursive direct scalar calls;
- scalar returns;
- functions whose runtime-contract requirements are satisfied by the selected
  profile.

Runtime-contract resolution rejects requirements that the selected profile does
not provide, including today's requirements for managed allocation, heap, GC,
roots, safe points, write barriers, string formatting, collections, classes,
closures, standard-library adapters, interface dispatch, coroutine/async
operations, arbitrary FFI, PLRI adapters, exceptions, and unwind.

The eBPF validator separately rejects floating point, recursion, invalid entry
signatures, unproven loops, indirect or dynamic calls, unsupported MIR
operations, incompatible layouts, and backend representations that this first
implementation cannot lower yet.

The memory model for the MVP is scalar SSA lowering backed by the `linux-ebpf`
profile. There is no selected profile implementation for a Pop managed heap,
object relocation, stack maps, standard runtime adapter, helper access, or map
access. Kernel memory, packet data, helpers, maps, ring buffers, and BTF are
future work that must be represented through explicit validated contracts
rather than raw pointer fallback.

Diagnostics use stable backend/target codes in the `POP7000` range and must
name the eBPF target, the rejected category, and the relevant MIR/source origin
when available. A failure emits no partial object.

## Consequences

- Pop Lang gains a real experimental path from source to ELF eBPF object while
  preserving backend-neutral HIR and MIR.
- The feature is explicitly selected and is not a default build backend or a
  `0.1.0` release requirement.
- The initial XDP example proves the pipeline, but not packet access, maps,
  helpers, BTF, CO-RE, ring buffers, tracepoints, or attachment.
- LLVM BPF availability is an environment capability, so tests that require
  object emission must detect and skip cleanly when unavailable; validation and
  target tests remain unconditional.

## Alternatives considered

### Custom eBPF instruction emitter

Rejected for the first slice because it would expand the PR into instruction
selection, relocation, ELF writing, verifier-oriented optimization, and target
testing. A custom emitter remains possible after the MIR subset and artifact
contract mature.

### Treat eBPF as a normal native executable target

Rejected because eBPF has no process entry and no ordinary executable artifact.
Accepting it through the native path would hide target/profile contract failures
and risk host-target object emission.

### Add source-level BPF attributes immediately

Rejected for the MVP. CLI selection is sufficient for one explicit XDP entry
without adding parser, resolver, type-checker, and metadata surface area. Source
attributes can be revisited when maps, helpers, context types, or multiple
program entries require source ownership.

## Required conformance tests

- target tests recognize `bpfel-unknown-none` and `bpfeb-unknown-none` as ELF
  LLVM BPF targets;
- runtime-contract tests prove `linux-ebpf` satisfies scalar contracts, rejects
  missing managed/runtime contracts with profile/target/origin detail, and is
  incompatible with non-BPF targets;
- validation accepts the minimal scalar XDP_PASS program;
- validation rejects floating point, missing runtime contracts, invalid
  signatures, recursion, indirect calls, allocation/runtime effects, and
  unproven loops;
- backend-private LLVM IR contains the BPF triple, an `xdp` section, the entry
  wrapper, and no Pop runtime symbols;
- object-emission tests verify ELF BPF headers, section, symbol, and
  deterministic output when LLVM BPF is available, and skip only that part with
  an explicit reason otherwise;
- CLI tests require explicit target/profile/program/output selection and reject
  unknown targets or runtime profiles without writing an object.

## Future work

Future ADRs or amendments must define typed contracts for checked integer trap
lowering, XDP context access, bounds-checked packet reads, helpers, maps, BTF,
CO-RE relocations, ring buffers, tracepoints, tail calls, verifier-oriented loop
bounds, and richer program types.

## Documents/components affected

Compiler pipeline, intermediate representations, backend architecture, CLI and
tooling contract, implementation roadmap, target inventory, diagnostic catalog,
LLVM backend tests, driver tests, and examples.
