# ADR 0052: Typed Results, Error Declarations, and Scoped Cleanup

- Status: accepted
- Date: 2026-07-13
- Supersedes: the typed-result and scoped-cleanup deferrals in ADRs 0030 and
  0032

## Context

ADR 0022 separates expected recoverable failures, runtime traps, and panic
unwinding, but it deliberately leaves the complete source workflow open. The
bootstrap prelude names `Result<T, TError>` without defining its cases, examples
assume `Result.Ok` and `Result.Error`, and the language does not yet define error
declarations, propagation, or the cleanup path taken by an early error return.

Postfix `?` is already the optional-only propagation operator from ADR 0051.
Extending it to results would obscure whether `nil` or a typed error controls an
exit and would reopen an accepted contract.

## Decision

### Result identity and construction

`Result<T, TError>` is the reserved nominal generic tagged union supplied by
`Pop.Standard` and included in the curated prelude. It has exactly these public
cases:

```luau
public union Result<T, TError>
    Ok(value: T)
    Error(error: TError)
end
```

The public source spellings are `Result.Ok(value)` and
`Result.Error(error)`. Generic case construction uses the expected result type
when that context determines every type argument. Otherwise source supplies
all arguments in the established Luau-directed form, such as
`Result.Error<<Player, LoadError>>(error)`. Partial, runtime, or best-effort
generic inference is not permitted.

`Result` is not a table, class, exception wrapper, or dynamically discovered
carrier. HIR and MIR retain its stable `BuiltinTypeId`, exact type arguments,
and stable case identities.

### Error declarations

Pop adds a Luau-shaped `error` declaration for a closed nominal family of
recoverable errors:

```luau
public error LoadError
    Io(error: Io.Error)
    InvalidData(message: String)
end
```

Visibility, generic parameters, case attributes, payload syntax, naming, and
exhaustive `match` behavior follow tagged unions. An error declaration has a
distinct `ErrorId` and `ErrorCaseId` in resolved and typed representations; it
is not silently interchangeable with an ordinary union. Its runtime
representation may share the verified tagged-layout mechanism because that
sharing is not observable and does not collapse the semantic identities.

An error value has no implicit base class, message field, numeric code, stack
trace, or string conversion. `Result<T, TError>` accepts any one statically
known `TError`; public APIs should use nominal error declarations when callers
need stable cases. This keeps small private results possible without an
operational dynamic escape hatch.

### Result propagation

The prefix expression `try expression` propagates one typed result:

```luau
public function loadName(path: Path): Result<String, LoadError>
    local player = try loadPlayer(path)
    return Result.Ok(player.name)
end
```

`try` evaluates its operand exactly once. The operand must be
`Result<T, TError>`, and the enclosing function must return exactly one
`Result<U, TError>` with the same resolved error type. `Ok(value)` continues as
an expression of type `T`; `Error(error)` exits through the active cleanup
chain and returns `Result.Error(error)`. The success types `T` and `U` need not
match.

There is no implicit error conversion, subtype conversion, `From` lookup, user
overload, catch, throw, or string-based adapter. A caller that changes the
error type must exhaustively match and construct the destination error case
explicitly. Postfix `?` remains optional-only.

### Matching boundaries

Recovery is an ordinary exhaustive `match` on `Result.Ok` and `Result.Error` or
on a nominal error declaration's cases. A Bubble boundary, entry point, task
boundary, or FFI adapter cannot leave an expected failure untyped or convert it
to panic implicitly. The existing missing-case diagnostic and safe arm
insertion apply to both result and error matches using stable identities.

### Scoped cleanup

`defer ... end` registers one lexical cleanup block when execution reaches the
statement:

```luau
local handle = try File.open(path)
defer
    File.close(handle)
end
```

Registered blocks run exactly once in last-in, first-out order when control
leaves their lexical scope by fallthrough, explicit return, result propagation,
`break`, `continue`, panic unwind, or cancellation. A branch that never reaches
the `defer` statement does not register it. Runtime traps are not catchable and
do not promise cleanup.

The initial synchronous cleanup body may contain typed ordinary statements and
calls but cannot contain `return`, `break`, `continue`, result propagation,
another `defer`, suspension, or cancellation. Its calls may panic; a panic
during normal cleanup begins ordinary unwinding, while a second panic during
panic cleanup replaces neither panic with ordinary recovery: PLRI reports the
closed `PanicKind.DoublePanic`, unwinding stops, and the nearest task/process
panic boundary terminates according to its existing policy. Payload objects are
not nested or retained by this terminal record, so double-panic handling cannot
allocate recursively. Async cleanup and cancellation-aware suspension remain
part of the structured-concurrency roadmap slice.

Cleanup is a compiler-owned lexical control-flow construct, not a closure,
finalizer, destructor, or runtime callback registry. It observes the
then-current typed bindings from its lexical scope. Captured managed values stay
precisely rooted until the cleanup has run.

### HIR, MIR, and backends

HIR has distinct typed nodes for result construction, `try`, error
construction/matching, and registered cleanup scopes. It records exact result,
success, and error types plus stable result/error case identities.

Canonical MIR lowers result handling to explicit `resultIsOk`, `resultGetOk`,
and `resultGetError` operations, conditional branches, typed block arguments,
and explicit `Result.Ok`/`Result.Error` construction. Each extraction is valid
only on a path dominated by the matching result test. A propagation error edge
is a named failure edge and reaches the function return only after traversing
every active cleanup block in last-in, first-out order.

Cleanup blocks are ordinary verified MIR blocks tagged with a typed
`CleanupScopeId` and one closed exit reason: `Normal`, `Return`,
`ResultFailure`, `Break`, `Continue`, `Unwind`, or `Cancellation`. Normal,
return, failure, loop-control, unwind, and cancellation edges name the first
required cleanup block; cleanup completion names the next cleanup or the
original destination. A chain stays in one scope for internal control flow or
moves toward an earlier registered scope, never the reverse. Panic cleanup ends
with `resumeCurrentUnwind`. Panic-capable calls retain ADR 0022's explicit
unwind action. Backends do not reconstruct cleanup from source.

The interpreter, optimized MIR, LLVM, and future VM use the same discriminant,
payload, failure-edge, cleanup-order, and root-liveness contract. Native layout
is private and may use the existing tagged-union representation; no error or
case name is consulted at runtime.

### Diagnostics and documentation

Diagnostics use stable structured codes and typed arguments for non-result
operands, invalid propagation contexts, mismatched error types, ambiguous
generic case construction, illegal cleanup control, and incomplete result/error
matches. A propagation fix is safe only when the exact result and error types
are already uniquely known. No fix inserts a cast, panic, dynamic lookup, or
implicit conversion.

For a function returning `Result<T, TError>`, `<returns>` documents only the
`Ok` value and every reachable public error case must be documented by an
`<error type="...">` entry, directly or through checked inherited
documentation. The `type` resolves to `TError` or one of its exact nominal
cases. `<error>` is rejected on functions without a result error type and can
never document panic. Public error declarations and cases require checked
summaries under the ordinary public-documentation completeness rules.

## Consequences

- Expected failures remain ordinary static data while gaining a complete,
  concise workflow.
- Optional and result propagation stay visually and semantically distinct.
- Error conversion is explicit at the one boundary where information changes.
- Cleanup order and failure exits are backend-neutral and verifiable.
- Error declarations gain stable semantic identities without creating an
  exception hierarchy or runtime reflection surface.

## Alternatives considered

### Reuse postfix `?` for results

Rejected because ADR 0051 deliberately reserves it for optional absence and
because one punctuation form would hide two different early-exit contracts.

### Add exceptions or `try`/`catch`

Rejected because expected failures are values, exhaustive boundaries are
static, and panic remains the separate invariant-failure mechanism.

### Convert errors implicitly during propagation

Rejected because conversion lookup would make a local expression depend on
hidden declarations and could discard information without an explicit match.

### Use finalizers for cleanup

Rejected because finalizer timing, resurrection, and collector dependence
contradict deterministic lexical resource management and the accepted GC
contract.

## Required conformance tests

- result/error declaration identities, visibility, generic cases, construction,
  and exhaustive matching;
- contextual and explicit result type arguments plus ambiguous-construction
  rejection;
- successful and error propagation, operand-once evaluation, exact error-type
  matching, nested calls, and optional/result operator separation;
- conditional registration, lexical fallthrough, return, failure, loop-control,
  panic, cancellation, LIFO order, and illegal cleanup-control rejection;
- checked `<returns>`/`<error>` resolution, missing/duplicate/foreign cases,
  inherited documentation, and `<panic>` separation;
- HIR/MIR verifier negatives for wrong result identities, unguarded payload
  extraction, skipped cleanup, duplicate cleanup, and mistyped failure edges;
- interpreter, optimized-MIR, and LLVM differential tests including managed
  payload rooting through cleanup; and
- explicit C-backend rejection where the experimental backend lacks support.

## Documents/components affected

Language model, syntax/nomenclature, type system, foundational library metadata,
resolver IDs, diagnostics, XML documentation, HIR, MIR, optimizers, interpreter,
LLVM, runtime unwind/cancellation policy, conformance matrices, and standard
library examples.
