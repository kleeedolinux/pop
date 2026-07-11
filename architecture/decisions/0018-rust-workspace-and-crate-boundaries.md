# ADR 0018: Rust Workspace and Crate Boundaries

- Status: accepted
- Date: 2026-07-10
- Supersedes: none

## Context

Milestone 0 requires an implementation language and repository layout before
compiler behavior is written. The component architecture already defines
ownership and dependency boundaries, but deliberately did not select a host
language. A monolithic initial executable would make those boundaries implicit,
encourage accidental dependency cycles, and make it difficult to locate the
component responsible for a behavior.

## Decision

The initial Pop Lang compiler, runtime, and first-party tools are implemented in
Rust using edition 2024 and a virtual Cargo workspace with resolver version 3.
The workspace's minimum supported Rust version starts at 1.85, the first stable
release supporting edition 2024, and may be raised deliberately with toolchain
and CI updates.

Each ownership boundary in the compiler component architecture has a focused
Cargo crate. Packages use the `pop-` prefix, and directories mirror the
architectural areas under `crates/compiler/`, `crates/runtime/`, and
`crates/tools/`. The user-facing executable is produced by `pop-driver` with the
binary name `pop`.

Workspace dependency declarations are centralized in the root `Cargo.toml`.
Member crates inherit shared package metadata and lints. Crates may depend only
in the direction authorized by the component architecture. In particular:

- foundation and runtime-interface contracts do not depend on semantic compiler
  or backend crates;
- source, syntax, project, resolution, type, compile-time, HIR, and MIR crates do
  not depend on LLVM or other backend implementations;
- HIR and MIR do not depend on backend implementation crates;
- backends consume verified MIR through backend-neutral contracts;
- orchestration and tools may compose lower-level crates but do not move their
  semantic ownership into the driver.

The initial skeleton uses the Rust standard library only. Adding a third-party
dependency requires a concrete component need, license/security review
proportional to its role, and tests proving the boundary it supports. Cargo
package/crate names are implementation details and do not replace Pop Lang's
`Item → Module → Bubble → Package → Workspace` terminology.

Repository architecture tests validate the member inventory, manifest
inheritance, required source targets, and forbidden dependency directions. New
feature work follows architecture, then failing tests, then implementation.

## Consequences

- Contributors can locate behavior by compiler phase and review dependency
  direction directly in manifests.
- Narrow crates allow focused unit tests and prevent backend types from leaking
  into portable compiler layers.
- Cargo builds more packages than a monolithic executable, but incremental builds
  can reuse stable lower layers.
- Some crates begin as documented empty boundaries and gain behavior only when
  their roadmap milestone starts.
- Rust is the host implementation language; it does not authorize Rust syntax or
  semantics in Pop source.

## Alternatives considered

### Start with one Rust crate and split it later

Rejected because dependency boundaries would be conventional rather than
machine-checkable during the period when the architecture is most vulnerable to
accidental coupling.

### Use modules inside one package for every component

Rejected because Rust module privacy does not make cross-component dependency
direction or feature ownership as visible and testable as package boundaries.

### Defer the host-language choice

Rejected because implementation is beginning and the existing Cargo skeleton
already requires an explicit decision to avoid architecture drift.

## Required conformance tests

- the root is a resolver-3 virtual workspace with the accepted crate inventory;
- every member inherits workspace package metadata and has a buildable target;
- every workspace path dependency resolves to a declared member;
- portable compiler crates have no dependency on backend implementations;
- foundation and runtime-interface crates have no forbidden higher-layer
  dependencies;
- `pop-driver` produces the binary named `pop`;
- the workspace builds and tests without undeclared external dependencies.

## Documents/components affected

Implementation roadmap, compiler component architecture, closed design
questions, repository agent policy, root Cargo manifest, compiler/runtime/tool
crate manifests, and architecture conformance tests.
