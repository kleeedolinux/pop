# ADR 0078: Native ABI 2 Writable-Root Coexistence

- Status: accepted
- Date: 2026-07-14
- Depends on: ADR 0038, ADR 0039, ADR 0070, and ADR 0077
- Supersedes: none

## Context

Native ABI 1.11 is the accepted stable-token bootstrap interface. Its GC safe
point accepts readable root values, and current LLVM lowering spills them but
continues to use the original SSA values after the call. Changing that pointer
to writable or changing the reported major version would not prove relocation;
it would silently make existing ABI 1 code appear compatible with a contract it
does not implement.

Pop needs an incremental transition in which bootstrap artifacts remain
unambiguous while ABI 2 writable-root lowering, native runtime composition, and
forced relocation are developed and verified together. The transition cannot
depend on C `const` spelling because pointer mutability is not a distinct link
identity.

LLVM's statepoint documentation requires every use after a relocating safe
point to consume a newly relocated value rather than the old SSA value. It also
documents explicit writable stack regions as an alternative representation,
while warning that frontend spill/fill correctness remains the frontend's
responsibility. See [Garbage Collection Safepoints in LLVM][llvm-statepoints]
and the [`gc.relocate` language reference][llvm-relocate].

[llvm-statepoints]: https://llvm.org/docs/Statepoints.html
[llvm-relocate]: https://llvm.org/docs/LangRef.html#llvm-experimental-gc-relocate-intrinsic

## Decision

### Closed native ABI descriptors

`pop-runtime-native-abi` owns two explicit descriptors during the transition:

- ABI 1.11, the bootstrap stable-token contract; and
- ABI 2.0, the first writable-root contract.

There is no ambiguous unqualified "current ABI" usable for profile selection.
The compiler selects a descriptor only after validating the requested runtime
profile, backend capabilities, and target capabilities. Existing ABI 1 symbols
and their meaning remain unchanged.

### Distinct writable safe-point entry

ABI 2 uses the distinct physical entry `pop_rt_gc_safe_point_v2`. It receives
the safe-point identity, a writable contiguous array of exact nonzero-or-null
`u64` managed-reference tokens, and its count. On success it writes every
possibly relocated token back in canonical `RootSlot` order before returning.
On failure it returns the closed failure status without exposing a partially
updated array.

All other ABI operations retain their accepted physical spellings where their
ABI 2 representation is unchanged. A backend cannot infer ABI 2 support merely
because those common symbols link.

The profile-independent entry `pop_rt_supports_abi(major, minor)` reports
whether the linked native facade implements that complete descriptor. The
stable conformance facade initially reports ABI 1.11 only even if test-only ABI
2 helpers are compiled. A production facade reports ABI 2.0 only after its
moving collector composition and writable-root postconditions pass. Profile
selection still fails before normal program entry; the query is a defensive
load/link check, not dynamic semantic dispatch.

### Backend-private reload lowering

LLVM ABI 2 lowering may use either verified `gc.statepoint`/`gc.relocate`
sequences or explicit writable `u64` root arrays. The initial Pop lowering uses
the explicit array because PLRI managed references are opaque whole-object
tokens with no source-visible derived pointers.

For every ABI 2 safe point, LLVM lowering must:

1. spill the current value of every exact live managed root into one canonical
   array;
2. call `pop_rt_gc_safe_point_v2`;
3. branch to failure handling when the call rejects the publication;
4. reload every array entry into a new backend-private SSA value; and
5. rewrite every observably later instruction, terminator, branch argument,
   loop backedge, and control-flow merge to use the reloaded value.

The original SSA value cannot appear on a path observably after that safe point.
Root aliases are backend-private state; HIR, MIR, PLRI, and Pop source do not
gain relocation instructions or native symbol names.

### Capability and staging rule

Emitting an ABI 2 call is not enough to advertise
`RelocatingManagedReferences`. LLVM keeps that capability disabled until all of
the following pass:

- emitted LLVM verification rejects post-safe-point uses of old root SSA
  values, including merges and loops;
- ABI 2 load/link validation proves the complete descriptor;
- a native test runtime changes every published token and rejects each old
  token; and
- forced relocation execution preserves exact results before and after MIR and
  LLVM optimization.

The experimental C backend remains unable to select either managed runtime
profile. The MIR interpreter keeps using mutable typed `RootPublication`
directly and needs no native ABI spelling.

## Consequences

- ABI 1 bootstrap artifacts remain link- and behavior-compatible.
- ABI 2 cannot be confused with a writable reinterpretation of the ABI 1
  symbol.
- The native facade may stage ABI 2 tests without falsely reporting production
  support.
- LLVM reload correctness is explicit across control flow rather than assumed
  from a mutable stack buffer.
- A single library can share unchanged physical helpers while profile
  validation still distinguishes complete ABI descriptors.

## Alternatives considered

### Change the ABI 1 root pointer from const to mutable

Rejected because C pointer mutability does not create a new link identity and
old LLVM code would still use stale SSA values.

### Bump the one global version constant immediately

Rejected because the stable native facade and current LLVM backend do not yet
satisfy ABI 2. Reporting major 2 first would violate fail-closed profile
selection.

### Return a relocation map

Rejected by ADR 0039 because it loses the canonical `RootSlot` relationship and
invites untyped token lookup.

### Require LLVM statepoints in the first slice

Rejected as the only allowed mechanism. Statepoints remain permitted, but the
explicit writable-array contract is simpler for Pop's opaque base tokens and
can be verified with direct spill/reload tests. It does not weaken the ban on
old post-safe-point SSA uses.

### Choose ABI symbols dynamically at runtime

Rejected because runtime string/symbol selection would turn a build/profile
contract into dynamic behavior. The compiler selects fixed symbols after typed
profile validation.

## Required conformance tests

- ABI 1.11 and ABI 2.0 descriptors are distinct and immutable;
- ABI 1 safe points preserve readable stable tokens and keep their original
  symbol;
- ABI 2 rejects null/nonzero-count or invalid arrays without partial writes;
- ABI 2 writes relocated tokens back in exact canonical slot order;
- the stable native facade reports ABI 1.11 support and rejects ABI 2.0 support;
- a production/test facade cannot report ABI 2 until every required entry and
  writable-root postcondition exists;
- emitted LLVM ABI 2 reloads every root into a new value and contains no old
  post-safe-point use on straight-line, branch, merge, or loop paths;
- forced native relocation changes tokens, invalidates old tokens, and
  preserves task/program results;
- ABI/profile mismatch fails before program entry without bootstrap fallback;
  and
- MIR, PLRI, the MIR interpreter, and source syntax remain backend-neutral and
  contain no ABI symbol or relocation alias.

## Documents/components affected

Runtime and ABI architecture, backend architecture, native ABI vocabulary,
native facade roots and identity, backend capability validation, LLVM private
lowering and verifier tests, linker/load checks, scheduler/GC stress profiles,
closed design decisions, roadmap, and architecture tests.
