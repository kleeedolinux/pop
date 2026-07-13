# ADR 0032: Concise APIs and Explicit Cost Contracts

- Status: accepted
- Date: 2026-07-12
- Depends on: ADR 0030 and ADR 0031
- Supersedes in part: ADR 0016 illustrative public names

## Context

A function/data-first library can still be cumbersome. Deep qualification,
repeated domain words, wrapper records, configuration ceremony, hidden copies,
and convenience layers with unclear cost would make Pop Lang feel heavier than
its Luau-shaped language surface. Avoiding OOP is necessary but insufficient.

## Decision

Public APIs optimize common call sites for clarity, brevity, static safety, and
predictable cost. A common operation normally needs one call after construction
of its input values; resource operations may add one explicit scope or close.
Advanced options use immutable records with useful safe defaults. Qualified
names do not repeat their namespace, and a namespace level must improve
discovery enough to justify its presence.

Every public operation documents its cost class where relevant: allocation,
copying or view lifetime, iteration strategy, blocking/suspension, buffering,
native transitions, dispatch, and complexity. Convenience functions lower to
the same typed primitives available to advanced callers. They may allocate or
copy only when their contract says so. Performance claims require benchmarks;
until measured, documentation states an intended bound rather than claiming a
result.

Views, slices, iterators, streams, and caller-owned buffers are first-class.
Adapters are statically resolved or explicitly carried typed values. Runtime
string registries and hidden dynamic dispatch are forbidden. Specialization is
permitted only through accepted generic/compile-time architecture and must not
change semantics across backends.

## Rationale

Pop Lang's lightweight surface is a product contract. Static safety and broad
capability do not require ceremony, and concise convenience does not require
hidden work. Reviewing the common call, the advanced call, and the efficient
primitive together prevents ergonomics and performance from being traded away
in separate layers.

## Usability rules

- Prefer `Json.decode(text, UserSchema)` to
  `Data.Json.Decoder.decodeValue(text, UserSchema)`.
- Prefer `File.read(path)` and `File.open(path, mode)` to service objects.
- Prefer `Http.send(request, options)` to a builder chain for ordinary requests.
- Prefer `Regex.find(pattern, text)` to a generic pattern object hierarchy.
- Prefer `Task.group(function(group) ... end)` to ambient task ownership.
- Use `parse`, `format`, `encode`, `decode`, `read`, `write`, `open`, `close`,
  `send`, and `receive` consistently within their distinct domains.
- Reserve `Client`, `Session`, `Connection`, `Transaction`, and similar nouns
  for actual stateful protocol/resource values.
- Reject `Builder`, `Factory`, `Manager`, `Provider`, `Service`, `Utility`, and
  `Helper` unless an independently reviewed semantic need cannot be named more
  directly.

## Cost model

- A view or slice is non-owning and does not allocate; its lifetime cannot
  outlive its owner.
- A consuming iterator is single-pass unless documented otherwise; adapters are
  lazy and allocation-free unless their captured state requires storage.
- A stream makes buffering and backpressure explicit. Convenience collection
  functions document their materialization.
- Caller-supplied buffers are reusable. A function that grows or replaces one
  reports that behavior in its signature/documentation.
- Sync calls do not schedule tasks. Async calls may allocate task state and must
  document suspension and cancellation points.
- Native/runtime crossings are explicit implementation effects and are batched
  where measurement shows transition cost matters.
- Typed protocol dispatch is direct, generic-specialized, or an explicit
  interface/function-value call; it never becomes name lookup.

## Consequences

- API review includes representative call sites and a cost table, not signatures
  alone.
- Short names are accepted only when namespace context makes them unambiguous;
  arbitrary truncation remains forbidden.
- A safe high-level API and its lower-level primitive share semantics and tests.
- Performance regressions become public-contract regressions once a measured
  budget is stabilized.

## Alternatives considered

### Optimize only namespace names

Rejected because verbose types, builders, repeated configuration, and hidden
allocation can make a shallow hierarchy just as cumbersome.

### Expose only low-level primitives

Rejected because forcing manual buffers and lifecycle code for every common
operation raises error rates and ceremony without improving the compiler's
ability to optimize.

### Promise zero cost for every abstraction

Rejected because async state, buffering, ownership transfer, and native calls
have real costs. Pop Lang documents and measures them instead of making an
unsupported slogan a compatibility promise.

## Compatibility, security, and portability

Existing bootstrap names are reviewed by call-site and cost tests before they
stabilize. Safe defaults and concise calls may not hide trust decisions,
capabilities, limits, or target restrictions. Portable cost semantics describe
allocation/copy/dispatch behavior; target-specific timing and throughput remain
measured properties rather than cross-target guarantees.

## Migration

Bootstrap APIs are reviewed through the same three call-site and cost table.
Once stabilized, a verbose or misleading form is deprecated only when an exact
replacement exists; automated rewrites cannot hide changes in allocation,
ownership, errors, authority, or target availability.

## Implementation impact

Each implemented API family requires concise default and advanced examples,
API-shape tests, allocation/copy tests where observable, complexity fixtures,
cross-backend semantic tests, and benchmarks for any stabilized performance
budget. Documentation tooling must render effect and cost metadata.

## Required conformance tests

- default, advanced, and efficient call-site snapshots;
- negative naming/API-shape checks for repeated context and ceremonial roles;
- allocation, copying, view lifetime, materialization, dispatch, task, and
  native-transition checks where observable;
- benchmark reproducibility and target-fact capture for measured budgets;
- cross-backend equality for portable convenience and primitive paths.

## Unresolved questions

- Syntax for async, cancellation, and suspension-capable cleanup. Synchronous
  typed-result propagation and lexical cleanup are fixed by ADR 0052.
- Which cost/effect facts become signature-level metadata versus checked docs.
- Generic specialization policy and its code-size controls.

## Documents/components affected

Public standard-library catalog and examples, API style, naming, XML
documentation, diagnostics/analyzers, HIR/MIR effect summaries, API baselines,
benchmarks, and architecture conformance tests.
