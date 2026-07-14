# Contributing to the Foundation Libraries

This guide is for contributors who want to improve Pop Lang's libraries without
first learning the entire compiler. **`Pop.Standard` is the recommended contribution path**
for almost everyone. It contains the public, portable APIs that ordinary Pop
programs use.

`Pop.Internal` is a trusted compiler/runtime boundary. Changes there can affect
intrinsics, memory safety, garbage collection, PLRI, every backend, and toolchain
compatibility. Contribute to it only when accepted architecture specifically
requires an Internal mechanism and you are prepared to prove those contracts.

The architectural authority for this area is:

- [Base libraries](../../architecture/16-base-libraries.md);
- [Public standard-library architecture](../../architecture/22-public-standard-library-architecture.md);
- [Modular base-library implementation](../../architecture/decisions/0035-modular-base-library-implementation.md);
- [Typed cross-Bubble function references](../../architecture/decisions/0036-typed-cross-bubble-function-references.md);
- [Typed Rust foundation adapters](../../architecture/decisions/0037-typed-rust-foundation-adapter-attribute.md).

## Choose the right contribution path

| What you want to add | Where it belongs | Experience needed |
| --- | --- | --- |
| Portable public function or algorithm | `standard/pop/src/` | Recommended starting point; normal typed Pop code |
| Test or documentation for a public family | `standard/tests/` and the owning architecture/library material | Recommended starting point |
| Temporary Rust prototype for an existing family | `standard/src/<family>.rs` | Rust plus the accepted public API contract |
| Accepted native-backed public adapter | An explicitly named `standard/src/` bridge module using `#[poplib]` | Native ABI, effects, bootstrap metadata, and cross-backend review |
| Portable trusted bootstrap helper | `internal/pop/src/` | Compiler bootstrap and visibility knowledge |
| Intrinsic, PLRI, GC, runtime, or capability bridge | The owning `internal/src/` service module | Advanced trusted-runtime work and architecture review |

When a function can be written safely and portably in Pop, put it in
`Pop.Standard`. Do not move it into `Pop.Internal` for performance speculation,
convenience, privileged access, or to bypass unfinished artifact work.

## Understand the directory layout

```text
crates/libraries/
├── standard/
│   ├── pop/src/       ordinary Pop.Standard Modules
│   ├── src/           temporary Rust prototypes and native adapters
│   └── tests/         focused tests by public API family
├── internal/
│   ├── pop/src/       trusted Pop.Internal Modules
│   ├── src/           intrinsic, PLRI, GC, and runtime adapters
│   └── tests/         focused trusted-boundary tests
├── bridge/            typed NativeExport descriptors
└── macros/            the Rust #[poplib] attribute implementation
```

There are still exactly two foundational Pop Lang Bubbles:
`Pop.Standard` and `Pop.Internal`. The Cargo crates and Rust modules above are
host implementation partitions; they do not create Pop Modules, namespaces,
Bubbles, or Packages.

Files below a `pop/src/` directory are real Pop Modules. Conventional discovery
finds additional `.pop` files automatically, so an ordinary Pop contribution
does not need a Rust `mod` declaration or compiler registry entry.

## Step-by-step: add a `Pop.Standard` function

### 1. Confirm that the API is authorized

Find the owning family in the
[standard-library catalogs](../../architecture/22-public-standard-library-architecture.md)
and read its accepted ADRs. A catalog entry marked `planned` is not permission
to silently choose its signature or behavior.

If the public contract is missing or ambiguous, stop and propose the
architecture first. Public names, errors, allocation, effects, complexity,
thread safety, and target availability are compatibility decisions.

### 2. Choose or create one Pop Module

Extend the existing family file under `standard/pop/src/`. For a newly accepted
family, create a readable `camelCase.pop` file with one file-scoped namespace.
Do not add the file to a central registry.

For example:

```pop
namespace Pop.Math

--- <summary>
--- Returns the smaller of two integers.
--- </summary>
---
--- <param name="left">
--- The first value.
--- </param>
---
--- <param name="right">
--- The second value.
--- </param>
---
--- <returns>
--- The smaller value.
--- </returns>
public function min(left: Int, right: Int): Int
    if left < right then
        return left
    end

    return right
end
```

Keep the code strongly typed and Luau-shaped. Use `camelCase` functions and
files, complete words, explicit public visibility, and checked XML
documentation. Do not introduce `Any`, `Dynamic`, runtime lookup, implicit
globals, JavaScript imports/exports, declaration braces, or lowercase
`snake_case` Pop identifiers.

### 3. Add the failing test before the implementation

Encode the accepted behavior before writing its body:

- positive behavior and boundary values;
- wrong types, invalid inputs, or inaccessible declarations;
- naming and documentation conventions;
- the relevant forbidden regression;
- differential/shared conformance when behavior executes across backends.

Place focused family tests beside the existing tests for that ownership area.
Use `crates/tools/test-runner/tests/foundation_sources.rs` for conventional
source-discovery and semantic-pipeline coverage. Run the new test against the
pre-feature tree and confirm that it fails for the intended missing behavior.

### 4. Implement the smallest conforming body

Implement only the accepted contract. An ordinary portable library function
must not require parser, resolver, HIR, MIR, backend, runtime, bootstrap-table,
or native-adapter edits.

If implementation appears to require one of those changes, pause and check
whether you are actually adding a compiler-known protocol, a native boundary,
or an API whose architecture is incomplete.

A new compiler-known identity, trusted intrinsic, or native ABI is not ordinary
library work. It requires its accepted architecture and the broader semantic,
runtime, and cross-backend tests described later in this guide.

### 5. Synchronize the public contract

Update checked documentation, examples, and the owning API baseline when one
exists. Remove contradictory old terminology or examples instead of leaving two
behaviors active.

### 6. Run focused and repository checks

Start with the narrow family/source checks, then run the architecture suite and
formatting checks listed below. Public semantic changes also require the owning
cross-backend and architecture-regression suites.

## Add a native-backed `Pop.Standard` adapter

Most Standard functions should not use Rust. Use a native adapter only when an
accepted contract requires a native transition, such as a PLRI-backed service
or the current bootstrap output boundary.

The Pop declaration or trusted bootstrap identity must exist first. Then:

1. Add a failing signature, descriptor, effect, and backend/linking test.
2. Put the Rust function in the owning, explicitly named bridge module.
3. Annotate its exact C ABI with `#[poplib(...)]`.
4. Add the generated descriptor to that module's explicit `NATIVE_EXPORTS`
   slice.
5. Verify that trusted bootstrap/reference metadata matches the descriptor.
6. Run native and cross-backend conformance tests.

```rust
#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "print",
    parameters(Int),
    results(),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_print_int(value: i64) {
    // Accepted native adapter body.
}
```

The attribute checks the Rust ABI and emits a typed descriptor. It does not
create a Pop declaration, assign a stable semantic ID, add a prelude name, or
authorize a new public API. Never replace the explicit inventory with directory
scanning, linker inventories, runtime registration, or string dispatch.

## Before proposing `Pop.Internal` work

Prefer `Pop.Standard`. Before editing `internal/`, all of these statements
should be true:

- an accepted architecture section or ADR requires the mechanism;
- the behavior cannot be an ordinary portable `Pop.Standard` function;
- the owning responsibility is specifically an intrinsic, PLRI adapter,
  GC/runtime transition, capability bridge, or bootstrap mechanism;
- the static signature, effects, failure/trap behavior, capabilities, and
  safe-point/GC behavior are defined;
- every backend can preserve the same semantic contract or reject a documented
  unsupported target capability;
- tests can deterministically prove the trusted boundary and its negative
  cases.

If any answer is no, do not implement the Internal change yet. Open the
architecture gap or move the ordinary functionality to Standard.

For authorized Internal work:

1. Read the relevant runtime, ABI, GC, intrinsic, and backend architecture.
2. Add failing positive, negative, proof-obligation, and cross-backend tests.
3. Use `internal/pop/src/` for portable trusted Pop helpers.
4. Use the owning `internal/src/` module only for host/runtime mechanisms.
5. Update the intrinsic, PLRI, capability, ABI, or GC schema that authorizes the
   behavior.
6. Use `#[poplib(bubble = Internal, ...)]` only for an accepted fixed native
   adapter and keep it in the explicit descriptor inventory.
7. Verify that private handles and Internal symbols never enter public reference
   metadata.

`Pop.Internal` cannot depend on `Pop.Standard`, cannot become a general helper
library, and cannot be referenced directly by user code. Unchecked operations
need the compiler proof or explicit unsafe caller contract required by the
accepted architecture.

## Current bootstrap limitation

New `.pop` Modules are conventionally discovered, reach verified HIR and MIR,
and can participate in the initial typed primitive-signature logical metadata
path. `pop build` now encodes arbitrary library contributions into verified
on-disk `.poplib` artifacts with checked documentation and a selected native
implementation. Dependency resolution still compiles local source and links
the corresponding object directly; consuming and linking the selected
implementation from a loaded `.poplib` remains unfinished.

Only the accepted `print(Int)` and `print(String)` bootstrap identities
currently execute through native Standard adapters. Do not turn a convenience
function into a compiler-known bootstrap identity merely to bypass this
remaining artifact-consumption work.

## Focused commands

For an ordinary Standard contribution, start with:

```text
cargo test -p pop-test-runner --test foundation_sources
cargo test -p pop-standard --test text
cargo test -p pop-standard --test api_baseline
cargo test -p pop-architecture-tests
cargo fmt --all -- --check
```

For native Standard or trusted Internal work, also run the applicable checks:

```text
cargo test -p pop-standard --test native_output
cargo test -p pop-internal --test runtime
cargo test -p pop-library-bridge
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Run any additional formatter, documentation, integration, linker, runtime,
stress, or cross-backend suite required by the owning architecture. Never claim
a check passed unless you actually ran it.

## Pull request checklist

- [ ] I found the accepted architecture that authorizes the behavior.
- [ ] I chose Standard unless the change genuinely requires an Internal
      trusted mechanism.
- [ ] I added a deterministic failing test before implementation.
- [ ] The implementation is in one focused family/service module.
- [ ] Pop source remains strongly typed, Luau-shaped, and canonically named.
- [ ] I did not add dynamic lookup, runtime registration, implicit globals, or
      backend-specific HIR/MIR behavior.
- [ ] Public documentation, examples, effects, errors, costs, and API baselines
      are synchronized.
- [ ] Native descriptors exactly match accepted metadata and stay in an
      explicit inventory.
- [ ] Standard/Internal dependency and visibility boundaries remain intact.
- [ ] I ran and reported every check proportional to the change.

If you are unsure whether work belongs in Standard or Internal, propose it as
Standard first and explain the capability or safety constraint that might
require a trusted boundary. That keeps ordinary contributions approachable and
makes exceptional Internal work explicit.
