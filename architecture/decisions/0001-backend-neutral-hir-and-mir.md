# ADR 0001: Backend-Neutral HIR and MIR

- Status: accepted
- Date: 2026-07-09

## Context

Pop Lang needs an initial native compilation path through LLVM and a credible
path to a future custom VM. If compiler semantics are encoded directly in LLVM
IR or if MIR mirrors LLVM instructions, a VM would need to reverse-engineer
source concepts or duplicate large parts of the front end.

## Decision

Pop Lang will use two backend-neutral semantic representations:

- typed HIR for resolved, language-level concepts;
- canonical MIR for explicit, portable execution semantics.

LLVM IR is output of the LLVM backend. It is not an upstream compiler IR and is
never referenced from HIR or MIR APIs. Backend-specific lower-level IRs are
allowed inside their respective backends.

Canonical MIR is accepted only after verification. Runtime interactions use
abstract PLRI operations, and target facts enter through a target query rather
than LLVM types or globals.

## Consequences

- Front-end and portable optimization work can be reused by native and VM
  backends.
- MIR needs its own verifier, printer, tests, and carefully specified semantics.
- Some LLVM optimizations may overlap with MIR optimizations.
- Backend integration requires an explicit lowering layer.
- A reference MIR interpreter can test semantics independently of LLVM.

## Alternatives considered

### Use LLVM IR as MIR

Rejected because it couples value representation, control flow, GC integration,
and operations to LLVM and makes a compact VM backend unnecessarily difficult.

### Lower HIR independently into every backend

Rejected because each backend would duplicate control-flow lowering, closure
conversion, checked casts, typed collection lowering, and other semantic work.

### Use only one high-level IR

Rejected because source-oriented transformations and low-level control-flow
optimization need different invariants and abstraction levels.
