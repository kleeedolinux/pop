# ADR 0074: Exact Source Function Overloads

- Status: accepted
- Date: 2026-07-14
- Depends on: ADRs 0019, 0029, 0036, 0054, and 0069
- Supersedes: the bootstrap-only overload limitation in ADR 0069

## Context

The declaration index already preserves multiple function Items with the same
namespace and name, and trusted bootstrap metadata selects `print(Int)` and
`print(String)` statically. Ordinary source functions cannot yet form a usable
overload set because signature resolution and call checking require one symbol
before parameter types are considered.

The standard library needs concise shared names across closed numeric and value
families, but adopting conversion rankings, return-type selection, dynamic
dispatch, or a foreign language's overload rules would create ambiguity and
hidden cost.

## Decision

Namespace-scope source functions may share a namespace and source name when
their complete parameter type packs differ. Result types, parameter names,
visibility, declaration order, effects, and owning Module do not distinguish
overloads. Two functions with equal parameter type packs are a duplicate
overload and fail before body checking, even when their result types differ.

The first source-overload slice accepts only non-generic candidates. A name may
still identify one generic function and use ADR 0054 inference, but a same-name
group containing a generic function is rejected until generic overload ranking
has a separate accepted contract.

For a call without explicit type arguments, lookup first selects exactly one
lexical/namespace/import group using existing visibility and shadowing rules.
The checker then:

1. keeps candidates with the exact value arity;
2. synthesizes each argument type once without inserting conversions;
3. keeps candidates whose parameter types exactly equal the argument pack; and
4. succeeds only when exactly one candidate remains.

No numeric conversion, optional lifting, subtype preference, generic ranking,
result context, declaration order, or runtime value participates. Zero matches
produce a typed no-matching-overload diagnostic listing candidate declarations.
Multiple matches are an architecture/compiler incident after duplicate checking
and fail closed.

A bare overloaded name is ambiguous as a function value in this slice. It is
not selected from an expected function type. Call syntax is the only accepted
selection site. Explicit type-argument syntax continues to require one generic
function and cannot select a non-generic overload group.

The selected `SymbolId` or referenced `SymbolIdentity` is stored in typed AST,
HIR, reference specialization metadata, and MIR. Backends never receive an
overload set and perform no selection.

## Consequences

- APIs may use one concise function name for exact closed types.
- Call cost remains one direct statically selected call with no runtime table or
  dispatch.
- Return-only overloading and implicit conversions remain forbidden.
- Lambda and context-dependent aggregate arguments may require an annotation
  until contextual overload solving is separately designed.
- `print` can later migrate to ordinary reference metadata without changing its
  exact-match source behavior.

## Alternatives considered

### Rank implicit conversions

Rejected because it hides conversion cost and requires a broad conversion and
tie-breaking policy that Pop Lang has not accepted.

### Select by expected result type

Rejected because calls become difficult to read and small surrounding changes
can silently change the target.

### Dispatch from runtime argument types

Rejected because Pop Lang has no dynamic fallback and MIR calls must already
name their target.

### Mangle public source names manually

Rejected because names such as `minimumInt64` repeat type context and damage
discoverability.

## Required conformance tests

- exact `Int`, `String`, and differing-arity overloads select stable symbols;
- the same overload group may span Modules in one Bubble;
- nearer namespace declarations shadow complete imported or prelude groups;
- equal parameter packs, including return-only differences, are rejected;
- wrong arity and wrong argument types produce deterministic diagnostics with
  candidate labels;
- generic/non-generic and multiple-generic overload groups are rejected;
- bare overloaded function values are rejected without expected-type guessing;
- public reference metadata preserves every overload and dependent Bubbles
  select by exact types;
- HIR/MIR and LLVM/interpreter behavior agree; and
- no conversion, dynamic lookup, runtime string dispatch, or backend selection
  appears.

## Documents/components affected

Type-system architecture, declaration/signature resolution, diagnostics,
reference metadata, body checking, HIR/MIR conformance, XML documentation
identity, standard-library naming guidance, and the roadmap.
