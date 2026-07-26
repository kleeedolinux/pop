# ADR 0110: Complete Modern Standard and Network Foundation

- Status: accepted
- Date: 2026-07-26
- Depends on: ADRs 0030, 0031, 0032, 0033, 0035, 0038, 0058, 0068,
  0081, 0096, and 0097
- Supersedes: the roadmap statement that the broad standard and network
  catalog is outside the required completion scope

## Context

The bootstrap foundation proves static typing, ordinary portable Pop Modules,
verified artifacts, selected native adapters, tasks, generated codec metadata,
and direct immutable Text/Bytes views. That is enough to compile representative
programs, but it is not the modern public foundation needed for users to build
general-purpose libraries without repeatedly inventing text, formats, I/O,
host, concurrency, security, and network primitives.

The domain catalogs already assign every public root to a tier and dependency
phase. They intentionally describe placement rather than implementation.
Without one bounded completion profile, “finish the standard library” could
mean either promoting a few prototypes or creating empty declarations for every
planned name. Neither result supplies a usable modern base.

## Decision

Pop Lang completes one machine-auditable modern foundation containing:

- every public root whose authoritative catalog tier includes `standard`; and
- the separately versioned official `Http` and `WebSocket` roots required to
  turn the standard `Net` and `Socket` foundations into an ordinary application
  protocol layer.

`Net` and `Socket` remain owned by the reserved `Pop.Standard` Bubble.
`Http` and `WebSocket` are owned by one independently versioned `Pop.Http`
Package and never enter the prelude or become implicit dependencies.

The exact root, Package, delivery group, completion status, and owning authority
are projected in
[`libraries/catalog/essential-libraries.tsv`](../../libraries/catalog/essential-libraries.tsv).
The projection is canonical UTF-8 TSV, sorted by public root, and versioned
independently from the complete placement inventory.

### Completion meaning

A family is `implemented` only when all of its accepted essential API contract
has:

1. exact public types, functions, errors, capabilities, and availability;
2. concise default, advanced, and efficient call-site examples;
3. checked allocation, ownership, copying, dispatch, blocking/suspension,
   native-boundary, limit, security, and complexity documentation;
4. deterministic positive, rejection, boundary, lifecycle, cancellation,
   security, and regression tests applicable to the family;
5. interpreter/LLVM differential proof for portable semantics and explicit
   capability rejection for unavailable targets/backends;
6. deterministic `.poplib`/Package artifacts and an append-only API baseline;
7. benchmarks for every stabilized performance claim; and
8. no contradictory prototype, native duplicate, hidden fallback, or planned
   declaration presented as executable.

`partial` means executable accepted behavior exists but the family contract is
not complete. `planned` means placement only. Neither status satisfies a
completion checkbox.

An empty namespace, placeholder record, always-unsupported function, metadata
row without an executable body, host-only helper, test-only implementation, or
unverified native wrapper cannot advance a family. The profile is a delivery
gate, not a namespace reservation list.

### Dependency order

Implementation follows these groups:

1. `core`: numeric values, randomness, identifiers, versions, URI/MIME,
   collections/sequences, bytes, Unicode, and text;
2. `formats`: codec/metadata, JSON/YAML/XML/CSV/TOML, regex/glob, locale,
   resources, and time;
3. `host`: I/O, paths, memory, files, directories, process, environment,
   platform, and terminal;
4. `concurrencyNetwork`: tasks, channels, atomics, actors, cryptography,
   sockets, and network transports;
5. `operations`: the standard telemetry contracts and deterministic no-op/test
   sinks; and
6. `networkProtocol`: the explicit `Pop.Http` Package with HTTP/1.1 and
   WebSocket essentials.

Later groups may be designed while earlier work proceeds, but an implementation
cannot bypass a missing value, lifetime, effect, PLRI, security, or capability
contract. Each public family with an architecture gap receives a focused
accepted ADR before tests and implementation.

### Optimization and portability

“Optimized” means evidence, not a label. Portable algorithms use ordinary Pop
source and canonical MIR. Compiler-known operations are added only when their
semantic role is accepted independently of one backend. Native/system work goes
through PLRI or a reviewed typed adapter with explicit effects and target
capabilities.

Common paths avoid avoidable allocation, copying, runtime registration, string
dispatch, reflective schemas, service graphs, and repeated native transitions.
Advanced views, buffers, streams, scopes, and typed options expose control
without replacing the concise call. Numeric throughput, parser throughput,
allocation, syscall/native transitions, scheduling latency, and protocol
tail-latency claims require reproducible target-labelled benchmarks.

## Consequences

- The broad modern foundation is required work rather than an optional catalog
  illustration.
- Users receive enough portable and host/network primitives to build normal
  Packages without depending on compiler-private or dynamic escape hatches.
- Completion is reviewable family by family and can be committed/reverted
  independently.
- `Pop.Standard` stays one reserved Bubble even though source/test ownership is
  split by family.
- `Pop.Http` stays explicit and independently versioned.
- Specialist official Packages, media, data engines, UI, science, AI, cluster,
  identity, devices, and vendor adapters remain outside this foundation unless
  another accepted decision adds them.

## Alternatives considered

### Treat the bootstrap foundation as the complete standard library

Rejected because it lacks the ordinary values, formats, I/O, host, concurrency,
security, and network contracts downstream libraries need.

### Add every catalog name as a stub

Rejected because planned declarations are not usable behavior and would
misrepresent implementation status, cost, security, and portability.

### Put HTTP and WebSocket in `Pop.Standard`

Rejected because ADRs 0030 and 0031 keep protocol/application layers separately
versioned. The standard network foundation supplies addresses, sockets,
transports, TLS contracts, and typed streams; `Pop.Http` composes them.

### Copy a foreign standard library surface

Rejected because capability breadth does not authorize foreign naming, dynamic
values, reflection, ambient authority, OOP service graphs, or backend-defined
semantics.

## Required conformance tests

- the essential profile contains exactly every `standard`-tier root plus
  `Http` and `WebSocket`, with no duplicate or extra root;
- every row has the exact owning Package, delivery group, status, and existing
  architecture authority;
- `Net`/`Socket` remain `Pop.Standard`, while `Http`/`WebSocket` remain
  `Pop.Http`;
- only `planned`, `partial`, and `implemented` are accepted profile statuses;
- `implemented` status requires matching public-root/API/artifact evidence;
- no family is advanced by a namespace shell, metadata-only declaration,
  unsupported placeholder, or host-only test helper;
- tier dependency direction, prelude boundaries, static typing, backend-neutral
  MIR, explicit effects/capabilities, and no-runtime-reflection invariants
  remain permanent regressions; and
- each completed family supplies the family-specific gates listed above.

## Documents/components affected

Public standard-library architecture and catalogs, implementation plan, closed
decisions, roadmap, architecture tests, `Pop.Standard`, `Pop.Http`, PLRI,
runtime/native adapters, API baselines, documentation, benchmarks, interpreter,
LLVM, target capability metadata, and Package artifacts.
