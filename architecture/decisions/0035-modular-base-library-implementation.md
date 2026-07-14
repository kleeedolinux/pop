# ADR 0035: Modular Base-Library Implementation

- Status: accepted
- Date: 2026-07-12
- Depends on: ADR 0009, ADR 0018, ADR 0024, ADR 0030, and ADR 0058
- Supersedes: none

## Context

The Rust bootstrap implementations of `Pop.Internal` and `Pop.Standard` began
as one `src/lib.rs` each. That was sufficient to prove the first native
boundary, but it makes unrelated library families share one source file and
one broad integration test. A contributor adding an ordinary typed algorithm
should not need to understand or edit compiler, HIR, MIR, backend, or runtime
orchestration code.

Splitting every library family into a Cargo crate would create the opposite
problem. It would confuse host implementation packages with Pop Lang Bubbles,
increase manifest and dependency work, and contradict the accepted two-base-
Bubble bootstrap model.

## Decision

The two reserved foundational Bubbles and their two Rust bootstrap crates stay
fixed. Each crate uses a thin `src/lib.rs` as an explicit module inventory, and
puts implementation in ownership-focused Rust modules:

- `pop-standard` partitions portable algorithms by the canonical public family
  they implement, such as `math.rs`, `text.rs`, and `sequence.rs`;
- native bootstrap adapters that are not ordinary public API families live in
  explicitly named bridge modules rather than beside portable algorithms;
- `pop-internal` partitions trusted code by runtime-service responsibility,
  such as `runtime.rs`, and does not become a catch-all helper crate;
- tests are partitioned by the same ownership names so a contributor can run
  and review one family independently.

Adding an ordinary portable library function changes its owning module,
focused tests, checked documentation, and the applicable API baseline. It does
not require edits to parsing, name resolution, HIR, MIR, a backend, or the
runtime. Compiler changes are justified only for an accepted compiler-known
protocol or semantic identity. `Pop.Internal` or PLRI changes are justified
only for a trusted intrinsic, capability, GC/runtime transition, or native
boundary.

The module inventory is explicit Rust source. Build scripts, filesystem glob
discovery, dynamic registration, string dispatch, and generated unchecked
registries are not used to hide ownership. Adding a new family requires one
clear module declaration; extending an existing family requires no central
source edit.

The Rust modules are bootstrap implementation partitions, not new Pop Lang
Bubbles, Packages, namespaces, or public compatibility promises. Normal
library algorithms migrate to ordinary `.pop` Modules as the verified
`Pop.Standard` source build becomes available, while retaining the same family
ownership and tests.

Repository-owned Pop source lives below each implementation crate in a
dedicated `pop/` root with `bubble.toml`, `src/lib.pop`, and ordinary additional
Modules under `src/`. This keeps Rust adapters and Pop source visually distinct
while preserving the conventional Package/Bubble layout inside the source
root. The manifests name the reserved identities and are toolchain build inputs;
they do not make either base library replaceable through an ordinary user
dependency.

Additional `.pop` Modules are discovered by the accepted Package convention,
not listed in Rust or compiler source. `Pop.Internal` is analyzed first;
`Pop.Standard` records one dependency on that verified Bubble. Repository
conformance analyzes every discovered Module through typed HIR and canonical
MIR. A contribution probe adds one extra typed Module to each source set and
must pass without changing a compiler registry, HIR operation, MIR operation,
or backend lowering.

ADR 0036 subsequently adds logical typed reference-metadata emission and
dependent-Bubble consumption for the initial primitive-signature slice. On-disk
`.poplib` encoding and implementation linking remain separate work. Portable
Rust bodies remain bootstrap evidence until their focused API contracts and
tests migrate to these Pop source Modules.

## Consequences

- Contributors can work on one library family without navigating the compiler
  pipeline or a monolithic foundation file.
- Review and test ownership follows public API families and trusted runtime
  services.
- The explicit inventory adds one small edit for a genuinely new family but
  keeps missing or accidental modules visible to review and architecture tests.
- Native adapters remain easy to locate and cannot be mistaken for portable
  standard-library algorithms.
- New Pop Modules require no central inventory edit and are verified through
  the same front-end and MIR contracts as user code.
- More source files are created, while Cargo package count and dependency
  direction remain unchanged.

## Alternatives considered

### One Cargo crate per library family

Rejected because Cargo crates are host implementation boundaries, not Pop Lang
Modules or Bubbles. This would increase contributor ceremony and weaken the
fixed base-library identity model.

### Automatically discover source files

Rejected for Rust implementation modules because build-time directory scanning
hides the reviewed host module inventory, adds generated state, and makes a
Cargo source layout depend on ambient filesystem discovery. Ordinary `.pop`
Modules use the separately accepted deterministic Package discovery contract;
that is language tooling behavior rather than hidden Rust code generation.

### Keep each base library in one source file

Rejected because unrelated APIs, native bridges, and tests would continue to
share one ownership surface as the library grows.

## Required conformance tests

- both foundation crates keep a thin explicit module inventory;
- standard portable families and native bridges occupy separate modules;
- internal trusted runtime services occupy focused modules;
- tests are independently runnable by family/service ownership;
- `pop-standard` depends on `pop-internal`, and the inverse remains forbidden;
- normal library modules do not acquire compiler or backend dependencies;
- both repository source roots discover every `.pop` Module through the
  conventional Bubble layout and reach verified HIR and MIR;
- `Pop.Standard` source records exactly one `Pop.Internal` Bubble dependency;
- adding a typed contribution Module needs no central source registry or
  compiler/backend edit;
- contributor documentation distinguishes ordinary algorithms from compiler-
  known identities, trusted intrinsics, and ABI changes.

## Documents/components affected

Compiler component architecture, base libraries, closed design questions,
architecture conformance tests, `pop-internal`, `pop-standard`, and foundation-
library contributor documentation.
