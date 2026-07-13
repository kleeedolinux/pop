# ADR 0047: Evaluated Runtime Constants

- Status: Accepted
- Date: 2026-07-13

## Context

Pop Lang already parses, types, evaluates, and exposes namespace `const`
declarations to compile-time tooling. Runtime function bodies, however, cannot
name those declarations even when their values are valid deterministic
compile-time values. This leaves the documented namespace constant form useful
only to attributes and declaration defaults.

Constants must not become mutable module storage, implicit globals, runtime
name lookup, or backend-specific data. Their runtime meaning also must not
depend on whether a backend happens to fold an expression.

## Decision

A resolved namespace constant may be used as a runtime expression. The front
end substitutes its already type-checked and deterministically evaluated value
into typed HIR before MIR construction. Each use retains the constant's exact
static type and source resolution still enforces Module/Bubble visibility.

The initial runtime-usable value set is the closed immutable constant set:
`nil`, `Boolean`, integers, floating-point values, `String`, and recursively
composed fixed tuples of those values. Aggregate values whose construction has
observable identity or mutation remain unsupported until separately designed.

The substitution creates ordinary backend-neutral literal/tuple HIR. It does
not create a runtime constant lookup operation, module storage cell, implicit
global, reflective registry, or new PLRI operation. String and tuple material
may still allocate according to their ordinary runtime representation.

Constants remain evaluated under the existing deterministic compile-time
budgets and capability restrictions. A runtime use cannot make an ineligible
initializer acceptable.

## Consequences

- Namespace constants become usable by ordinary runtime functions without
  introducing mutable module state.
- MIR and every backend observe the same already-evaluated typed value.
- Constant identity is semantic declaration identity at compile time, not a
  runtime address or lookup key.
- Constant-to-constant dependencies, public reference-metadata projection, and
  identity-bearing aggregate constants require their own complete integration
  before those cases are accepted.

## Rejected alternatives

### Allocate one runtime global per constant

Rejected because immutable constant values do not require mutable storage or
module initialization order, and address identity would add an unnecessary
observable contract.

### Resolve constants by source name in MIR or the runtime

Rejected because runtime string resolution is forbidden and would make a
front-end semantic fact backend-dependent.

### Re-type the initializer at every use

Rejected because it duplicates compile-time work, can repeat effects, and loses
the declaration's single evaluated value and provenance.

## Conformance requirements

Tests must cover:

- inferred and explicitly typed primitive constants used by runtime functions;
- deterministic compile-time calls folded before runtime HIR;
- exact type preservation and visibility-aware symbol resolution;
- rejection of unsupported identity-bearing constant shapes;
- verified HIR/MIR and matching interpreter/LLVM behavior;
- absence of runtime name lookup, global mutation, or new backend-specific IR.
