# ADR 0020: Nominal Interface Surface and Dispatch

- Status: accepted
- Date: 2026-07-10
- Supersedes: none

## Context

The architecture already requires nominal interfaces and explicit class
implementation, but did not define the first source surface or portable slot
contract. That gap prevents source, HIR, MIR, and runtimes from proving the same
dispatch behavior.

## Decision

An interface declaration contains typed instance method signatures. Interface
members are public by definition and omit redundant visibility and an owner
prefix:

```luau
public interface Reader
    function read(count: Int): String
end
```

The initial interface language has no fields, static members, default method
bodies, marker-only declarations, or interface inheritance. Those additions
require separate design.

A class explicitly names nominal interfaces after `implements`:

```luau
public class FileReader implements Reader, Closeable
    public function FileReader:read(count: Int): String
    end

    public function FileReader:close()
    end
end
```

For each named interface, the class must provide one accessible receiver method
with the same name and exact parameter/result types. Shape alone never creates
implementation. Interface order and method slots are canonicalized by stable
interface/member identity, not source spelling or runtime strings.

Class-to-interface conversion is a statically checked implicit upcast. A method
call through an interface-typed receiver resolves to an `InterfaceMethodId` and
an interface dispatch category. HIR/MIR carry the static interface type, slot,
receiver, argument types, result types, and effects. Runtime representation may
use a witness/dispatch table, but lookup is by verified identity/slot only.

## Consequences

- Interface dispatch is portable and independently verifiable.
- Adding a required interface method is an API-breaking change unless a future
  default-member design explicitly says otherwise.
- Function values remain preferred for a single local capability; interfaces
  serve real nominal polymorphic boundaries.

## Alternatives considered

### Structural or duck-typed implementation

Rejected because nominal implementation is already accepted and runtime duck
typing is a Lua regression.

### `class Name: Interface` punctuation

Rejected because it conflates implementation with type annotation and leaves
future single implementation inheritance visually ambiguous.

### Runtime method-name lookup

Rejected because it violates strong typing and backend-neutral resolved IR.

## Required conformance tests

- parsing and resolution of interface declarations and `implements` clauses;
- exact successful implementation and diagnostics for missing, duplicate,
  static, inaccessible, or mismatched methods;
- rejection of shape-only implicit implementation;
- class-to-interface upcast and interface-call typing;
- HIR/MIR verifier rejection of wrong interface, slot, receiver, arguments,
  results, or effects;
- direct/interface dispatch differential tests and no-string-lookup regression
  tests.

## Documents/components affected

Language model, syntax, type system, HIR/MIR, runtime metadata, interpreter,
reference metadata, diagnostics, and conformance tests.
