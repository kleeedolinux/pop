# ADR 0009: Two Base Libraries

- Status: accepted
- Date: 2026-07-10
- Superseded in part by: ADR 0030 (public standard-library direction)

## Context

Pop Lang needs a minimal trusted compiler/runtime layer and a broad coherent
public standard library. Combining them would expose GC/intrinsic implementation
details or force the runtime core to depend on high-level APIs. The .NET private
core-library plus public base-library layering demonstrates the value of this
separation.

## Decision

> **Supersession note:** ADR 0030 replaces this record's public BCL framing and
> public-library shape. This ADR continues to authorize the two reserved base
> library identities and their dependency direction.

The toolchain provides exactly two reserved foundational library Bubble identities:

- `Pop.Internal`, a private trusted library Bubble containing primitive implementations,
  intrinsic declarations, GC/runtime bridges, coroutine/loader/FFI transitions,
  and platform adapters;
- `Pop.Standard`, the public automatically referenced native Pop library Bubble
  organized under documented `Pop.*` namespaces.

User code cannot reference `Pop.Internal`. Compiler binding uses versioned
intrinsic IDs/signature hashes, not names. `Pop.Standard` depends on
`Pop.Internal`; the inverse dependency is forbidden.

All source-visible types and attributes, including built-ins, use `PascalCase`.
There is no universal `Object`, general reflection, dynamic value, or exception-
heavy expected-error model.

## Consequences

- Compiler/runtime bootstrapping needs a small primitive schema and intrinsic
  verification step.
- The public standard base can evolve/test independently of low-level implementation
  placement.
- Standard-library reference metadata stays public and stable while internal
  layouts can change with the compiler/runtime ABI.
- Normal projects automatically reference `Pop.Standard` and receive the fixed
  curated `Pop` prelude; external namespaces still use explicit `using`.
- API baseline, layering, target-profile, and cross-backend conformance checks
  become release gates.

## Alternatives considered

### One monolithic standard/runtime library

Rejected because it exposes layering details, enlarges the trusted core, and
makes bootstrap/versioning harder.

### Many independently versioned foundational libraries initially

Rejected for the initial architecture because it complicates the compatibility
surface before APIs stabilize. Public namespaces remain separable internally so
future packaging can change without source-name churn.
