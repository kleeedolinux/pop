# ADR 0058: Standard Foundation API Baseline

- Status: accepted
- Date: 2026-07-14
- Depends on: ADRs 0030, 0031, 0032, 0035, 0037, 0051, 0052, 0053,
  and 0057
- Supersedes: the unresolved exact-prelude decision in ADR 0031 and the public
  library implementation plan

## Context

`Pop.Standard` already has stable bootstrap identities, two typed `print`
overloads, collection and iteration contracts, ordinary Pop source for
`Sequence`, and accepted task/cancellation identities. The architecture still
left the exact prelude list unresolved and did not define one reviewable API
baseline. That allowed bootstrap metadata, catalog status, documentation, and
the resolver's implicit surface to drift independently.

A catalog entry is placement, not implementation. The baseline therefore must
distinguish names that are available now from planned roots, and it must not
turn prototypes into stable compatibility promises.

## Decision

### Exact initial prelude

The trusted `Pop.Standard` prelude contains exactly these source bindings:

| Kind | Bindings |
| --- | --- |
| Primitive types | `Boolean`, `Int8`, `Int16`, `Int32`, `Int64`, `UInt8`, `UInt16`, `UInt32`, `UInt64`, `Int`, `Float32`, `Float64`, `Float`, `Byte`, `String`, `Never` |
| Foundation types | `Bytes`, `Array`, `Table`, `Result`, `List`, `Set`, `Range`, `Task`, `CancelToken`, `Guid`, `Iterable`, `Iterator`, `Equal`, `Order`, `Hash`, `Close`, `AsyncClose`, `Iteration` |
| Namespace roots | `Sequence` |
| Function overload sets | `print(Int)`, `print(String)` |
| Trusted attributes | `CompileTime`, `AttributeUsage`, `AttributeValidator` |

`nil` remains a literal and is not a type binding. Optional values use the
language type `T?`; the initial prelude does not add a nominal `Option<T>` that
would duplicate that representation. Documentation may call this the optional
value contract, but it must not promise an `Option` runtime wrapper.

Prelude bindings retain the lowest resolution priority. A local declaration,
the current namespace, or an explicit `using` alias wins. Only metadata loaded
from the reserved toolchain `Pop.Standard` identity can provide these implicit
bindings. Copying a source spelling never grants prelude or compiler status.

`Sequence` is the sole implicit namespace root in the initial baseline. Its
members remain qualified (`Sequence.map`) unless an ordinary explicit `using
Pop.Sequence` brings them into unqualified lookup. No other catalog root is
implicit.

### Stable identities and baseline encoding

The bootstrap type, function, and compiler-attribute IDs remain their existing
stable semantic identities. `CancelToken` receives the next append-only
foundational type ID. Reordering a file cannot renumber an identity.

`libraries/standard/bootstrap/api-baseline.tsv` is the versioned,
machine-readable compatibility snapshot for the currently available public
surface. Every row records:

- a stable API identity;
- owner Bubble and namespace;
- complete source spelling and static signature;
- distribution tier and implementation status;
- prelude membership; and
- the owning documentation/cost contract.

The encoding is UTF-8 TSV with one fixed header, no escaping, no empty semantic
fields, ascending stable IDs within each kind, and one terminal newline. Values
that would require tabs or newlines are invalid rather than escaped. The loader
rejects unknown kinds, tiers, statuses, owners, duplicate identities, duplicate
signatures, noncanonical order, and a disagreement with trusted bootstrap
metadata.

Stable IDs are append-only within a schema version. Changing an existing
signature, owner, tier, or prelude flag is an API compatibility change. A
prototype row records real executable evidence but is not a stable-release
promise. Planned catalog entries never appear in this file. A public API can be
marked `implemented` only after its checked documentation, examples, cost
contract, artifact body, and interpreter/LLVM conformance are complete.

### Initial API baseline

The first baseline contains:

- the two typed `print` overloads as native bootstrap prototypes;
- `Sequence.map`, `Sequence.filter`, `Sequence.fold`, and
  `Sequence.collect` as portable Pop prototypes; and
- the exact prelude type and attribute identities needed to type those APIs and
  the accepted foundation contracts.

Rust-only Math, Text, and eager Sequence test helpers are implementation
prototypes, not Pop public declarations. They do not enter the public API
baseline. Planned catalog roots remain visible only through the catalog's
explicit status inventory.

## Consequences

- Ordinary projects receive one exact, testable implicit surface.
- Optional syntax and a nominal `Option` wrapper cannot drift into two active
  models.
- Catalog placement, executable prototype status, and stable API status remain
  distinguishable.
- Adding a prelude binding or changing a stable row requires compatibility,
  shadowing, collision, documentation, and baseline review.
- The namespace-root mechanism is narrow and does not become implicit import or
  runtime loading.

## Alternatives considered

### Treat every standard catalog root as prelude

Rejected because the catalog is broad and mostly planned. That would create
collisions and expose unavailable APIs as if they were implemented.

### Add a nominal `Option<T>` wrapper

Rejected for the initial release because `T?` already has accepted language,
HIR, MIR, and backend semantics. A second operational representation would add
conversion and identity questions without improving static absence handling.

### Derive the baseline from Rust module exports

Rejected because Rust modules are host implementation partitions. They do not
authorize Pop declarations, stable IDs, prelude membership, or compatibility.

### Use runtime registration

Rejected because the public surface must be deterministic before execution and
must never depend on string lookup or initialization order.

## Required conformance tests

- exact prelude binding and namespace-root snapshots;
- stable type/function/attribute/API ID and signature snapshots;
- malformed, duplicate, reordered, unknown-status, and bootstrap-disagreement
  baseline rejection;
- local/current-namespace/explicit-alias shadowing over prelude candidates;
- no implicit catalog roots other than `Sequence`;
- no nominal `Option`, `Any`, `Dynamic`, runtime registration, or string
  dispatch;
- catalog status and API baseline consistency; and
- checked documentation plus interpreter/LLVM evidence before any row advances
  from `prototype` to `implemented`.

## Documents/components affected

Base libraries, public standard-library architecture and implementation plan,
closed design questions, bootstrap metadata, type/resolution tests, library API
baseline tooling, architecture conformance tests, and the release roadmap.
