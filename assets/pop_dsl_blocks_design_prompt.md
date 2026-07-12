# Prompt: Design Idiomatic, Static, Safe, and High-Performance DSL Blocks for Pop

You are designing a first-class block and DSL system for **Pop**, a statically typed, general-purpose native language inspired by the lightweight syntax of Lua and Luau.

The goal is not to copy Ruby or Crystal. Study what makes their block syntax expressive, especially trailing blocks, `yield`, concise collection transformations, builder-style DSLs, and zero-cost abstraction, then redesign those ideas so they fit Pop's stricter model:

- statically resolved;
- less dynamic than Ruby or Crystal;
- safe by default;
- predictable to read and compile;
- friendly to optimization;
- suitable for native code;
- compatible with Pop's HIR → MIR → LLVM architecture;
- useful for embedded domain-specific languages without requiring macros, runtime reflection, string member lookup, `method_missing`, or universal dynamic objects.

The result should feel like a natural evolution of Pop, not Ruby syntax transplanted into another language.

---

## 1. Language context

Assume the following properties of Pop:

- Pop is strongly and statically typed.
- There is no source-visible `Any` or `Dynamic` escape hatch.
- Names and calls are resolved before backend lowering.
- Integer and floating-point operations have concrete types.
- Records, classes, interfaces, namespaces, modules, arrays, and typed tables have distinct semantics.
- Pop favors explicit, complete, readable names.
- Pop allows useful syntax sugar only when different surface forms lower to the same clear semantic model.
- HIR removes surface sugar and preserves typed language semantics.
- MIR is backend-neutral, explicit, and contains only executable logic.
- LLVM must not rediscover or redefine language semantics.
- Safety checks may only be removed when the compiler proves that doing so preserves behavior.
- Diagnostics must be deterministic and precise.

A typical Pop program currently looks like this:

```pop
namespace Example

private function fibonacci(value: Int): Int
    if value < 2 then
        return value
    end
    return fibonacci(value - 1) + fibonacci(value - 2)
end

function main()
    local value = fibonacci(28)
    print(value)
end
```

Design the block and DSL system around this visual identity.

---

## 2. Primary design objective

Create an idiomatic abstraction system with two deliberately separate concepts:

1. **Blocks**
   - lexical, non-escaping code passed to a call;
   - optimized as a zero-cost control-flow abstraction;
   - statically typed;
   - normally specialized and inlined;
   - never represented as a dynamic callable object;
   - ideal for iteration, resource management, builders, configuration, parsing, testing, transactions, and embedded DSLs.

2. **Procs**
   - explicit first-class callable values;
   - may be stored, returned, captured, or passed through unknown boundaries;
   - have visible capture, lifetime, allocation, and effect semantics;
   - may use a function pointer, environment object, stack closure, or heap closure depending on proven lifetime;
   - must never be created implicitly merely because a block captures a value.

The syntax and diagnostics must make the performance and lifetime boundary between a block and a proc obvious.

A reader should be able to look at source code and know whether the abstraction is:

- statically specialized;
- non-escaping;
- potentially allocating;
- dynamically dispatched;
- effectful;
- capable of non-local control flow.

---

## 3. Proposed core syntax

Explore a syntax close to the following, but improve it where necessary.

### 3.1 Declaring a function that accepts a block

```pop
function twice(&block: () -> Void)
    yield
    yield
end
```

Or, when the block name is used directly:

```pop
function applyTwice(value: Int, &transform: (Int) -> Int): Int
    local first = transform(value)
    return transform(first)
end
```

The proposal must decide whether Pop should support both implicit `yield` and direct block invocation.

Preferred direction:

- a function must declare a block parameter explicitly;
- merely writing `yield` must not silently change the function signature;
- `yield` refers only to the single trailing block parameter;
- when multiple block parameters are ever supported, they must be invoked by name;
- public API signatures must always expose block input types, result types, effects, and escape behavior.

Example:

```pop
function twice(&block: () -> Void)
    yield
    yield
end

twice do
    print("Hello")
end
```

### 3.2 Typed block parameters

```pop
function transform(value: Int, &block: (Int) -> Int): Int
    return yield value
end

local result = transform(3) do |value: Int|
    value + 2
end
```

Type annotations inside the block may be optional when fully inferred:

```pop
local result = transform(3) do |value|
    value + 2
end
```

The block's arity must be exact.

Unlike Crystal, Pop should not silently allow a block to ignore extra yielded arguments by declaring fewer parameters. Ignored values should be explicit:

```pop
pairs do |_, value|
    print(value)
end
```

This improves refactoring safety and prevents accidental acceptance of a changed callback contract.

### 3.3 Expression form

Permit a compact form only for a single expression:

```pop
local names = users.map { |user| user.name }
```

The compact form and `do ... end` must have identical semantics.

Avoid Ruby's left-most versus right-most binding distinction. Pop should have one deterministic attachment rule:

- a trailing block belongs to the immediately preceding call expression;
- parentheses may be omitted only when parsing remains unambiguous;
- formatter output should use parentheses whenever omission could confuse a reader.

These should be equivalent:

```pop
users.map() do |user|
    user.name
end

users.map do |user|
    user.name
end

users.map { |user| user.name }
```

This should not require a special precedence rule that changes between braces and `do ... end`.

### 3.4 Tuple yield and destructuring

Multiple values should use an ordinary typed tuple, not a special multi-yield mechanism:

```pop
function pairs(&block: ((Int, String)) -> Void)
    yield (1, "one")
    yield (2, "two")
end

pairs do |(number, word)|
    print(number)
    print(word)
end
```

Require explicit tuple destructuring.

Do not add magical auto-splatting based on arity. The same tuple rules should apply in normal assignments, parameters, pattern matching, and blocks.

Nested destructuring may be supported through existing pattern syntax:

```pop
values.each do |(left, (middle, right))|
    consume(left, middle, right)
end
```

---

## 4. Static scoped receivers for DSLs

Ruby DSLs often depend on changing `self`, implicit receiver lookup, open classes, and dynamic missing-method behavior. Pop must not copy those semantics.

Instead, design **typed scoped receiver blocks**.

A function may declare that its block receives a statically known DSL receiver:

```pop
function httpServer(
    name: String,
    &configuration: scoped ServerBuilder () -> Void
): Server
```

The call may then use receiver shorthand:

```pop
local server = httpServer("api") do
    .listen(8080)
    .workers(4)

    .route(.get, "/health") do
        .respond(Status.ok, "healthy")
    end
end
```

Rules:

- The leading `.` means “resolve this member on the current scoped receiver.”
- A bare name still follows ordinary lexical resolution.
- There is no fallback from a missing lexical name to the receiver.
- There is no runtime string lookup.
- There is no `method_missing`.
- The receiver type is known from the block parameter contract.
- Every member call is resolved and type-checked before HIR.
- Nested scoped blocks may install a new current receiver.
- The leading dot keeps receiver lookup visible even in deeply nested DSL code.
- The explicit form must always remain available.

Equivalent explicit form:

```pop
local server = httpServer("api") do |server|
    server.listen(8080)
    server.workers(4)

    server.route(.get, "/health") do |route|
        route.respond(Status.ok, "healthy")
    end
end
```

The proposal must decide whether the scoped and explicit forms can share one function signature or whether the signature must mark the block as `scoped`.

Preferred direction:

```pop
&configuration: scoped ServerBuilder () -> Void
```

This is stronger than an ordinary `(ServerBuilder) -> Void` block because it authorizes receiver shorthand while preserving the same underlying typed argument.

HIR should erase receiver shorthand immediately:

```text
call ServerBuilder.listen(receiver, 8080)
```

No special receiver concept should survive into MIR.

---

## 5. DSL examples the design must support

### 5.1 HTTP routing

```pop
namespace WebApplication

function main()
    local application = webApplication("example") do
        .http do
            .listen("127.0.0.1", 8080)

            .route(.get, "/users/:id") do
                .parameter("id", Int)

                .handle do |request|
                    local userId = request.parameter[Int]("id")
                    return Response.json(loadUser(userId))
                end
            end
        end
    end

    application.run()
end
```

Requirements:

- route parameters become typed through ordinary APIs or compile-time-known schema objects;
- no stringly typed dynamic field access;
- invalid nesting is rejected statically when possible;
- duplicate or contradictory configuration should produce precise diagnostics;
- handlers are blocks when they do not escape and procs only when the runtime must retain them.

The implementation may lower a retained handler to an explicit proc at a clear ownership boundary. This conversion must be visible in the API or syntax and must not happen silently.

### 5.2 Build configuration

```pop
local target = executable("popc") do
    .sourceDirectory("compiler")
    .optimization(.release)
    .linkLibrary("llvm")

    .platform(.linux) do
        .define("POP_LINUX")
    end
end
```

### 5.3 Parser construction

```pop
local parser = grammar("expression") do
    .rule("number") do
        .match(Token.integer)
        .produce do |token|
            Expression.integer(token.integerValue())
        end
    end

    .rule("addition") do
        .sequence(.reference("expression"), Token.plus, .reference("expression"))
        .produce do |left, _, right|
            Expression.add(left, right)
        end
    end
end
```

The design should explain which blocks execute during construction and which become retained parsing actions.

### 5.4 Resource management

```pop
withFile("data.txt", .read) do |file|
    local contents = file.readAll()
    print(contents)
end
```

The block must not escape the resource lifetime.

The compiler should reject:

```pop
local leaked = withFile("data.txt", .read) do |file|
    return proc do
        file.readAll()
    end
end
```

unless the file object is explicitly moved into an independently owned resource with valid lifetime semantics.

### 5.5 Transactions

```pop
database.transaction do |transaction|
    transaction.insert(user)
    transaction.update(account)
end
```

The block may return a typed value:

```pop
local user = database.transaction do |transaction|
    return transaction.loadUser(userId)
end
```

The proposal must define whether `return` exits the enclosing function or the block. Prefer one unsurprising rule and keep it consistent everywhere.

Recommended rule:

- the last expression is the block result;
- `next value` exits only the current block invocation and becomes the result of that invocation;
- `return value` exits the surrounding named function, as it already does elsewhere;
- `break` remains loop-only;
- blocks do not gain implicit non-local break semantics;
- early termination of a block-driven API should use an explicit typed return protocol or a future opt-in control effect.

This is safer than Ruby or Crystal's implicit non-local block control flow.

### 5.6 Tests

```pop
suite("Array") do
    .test("preserves insertion order") do
        local values = [3, 1, 2]
        expect(values).toEqual([3, 1, 2])
    end

    .test("rejects an invalid index") do
        expectTrap(.indexOutOfBounds) do
            local values = [1, 2]
            print(values[4])
        end
    end
end
```

The DSL must remain ordinary typed library code. The compiler must not need a special “test language” mode.

---

## 6. Block result semantics

A block is an expression with one statically known result type.

```pop
local doubled = transform(4) do |value|
    value * 2
end
```

The last expression may produce the result.

However, Pop should avoid silently synthesizing broad unions from unrelated branches merely because Ruby or Crystal would.

Require ordinary Pop control-flow typing:

```pop
local result = choose() do |value|
    if value > 0 then
        value
    else
        0
    end
end
```

If branches produce incompatible types, the programmer must use an explicit sum type, optional type, interface, or conversion.

Do not infer a dynamic or open-ended union.

For a `Void` block, unused final expressions should either be accepted and discarded according to existing Pop rules or diagnosed consistently with other `Void` contexts.

---

## 7. Blocks versus procs

Define the semantic distinction precisely.

### 7.1 Block

A block:

- is lexically attached to a call;
- cannot be stored in a local variable;
- cannot be returned;
- cannot outlive the call;
- may borrow local values;
- may mutate captured locals only under Pop's ordinary mutability and borrow rules;
- is statically specialized at the call site;
- introduces no required heap allocation;
- introduces no required indirect call;
- is represented in HIR as structured callable control flow;
- is lowered before or during MIR construction into direct control flow or a private specialized helper.

Example:

```pop
values.each do |value|
    print(value)
end
```

### 7.2 Proc

A proc is explicit:

```pop
local transform = proc |value: Int| -> Int
    value * 2
end
```

A proc:

- is a first-class value;
- has an explicit callable type;
- may capture;
- may escape;
- may allocate;
- may require indirect invocation;
- may implement an interface such as `Proc[(Int) -> Int]`;
- carries ownership and lifetime information;
- must expose effects when Pop's source effect system supports them.

Passing a proc:

```pop
local result = values.mapProc(transform)
```

Or, if APIs accept either form, the conversion must be explicit:

```pop
registerHandler(proc |request: Request| -> Response
    handle(request)
end)
```

Do not silently promote a block to a proc because a callee stores it.

A function that retains a callable must request a proc in its signature.

This must be a compile-time error:

```pop
function register(&handler: (Request) -> Response)
    globalHandlers.add(handler)
end
```

The diagnostic should explain:

> block parameter `handler` is non-escaping and cannot be stored; declare a proc parameter if the callable must outlive the call

---

## 8. Inlining and performance contract

Blocks should be designed as zero-cost abstractions, but the specification must avoid promises that are impossible to preserve under all compilation modes.

Define a semantic contract stronger than “the optimizer will probably inline it”:

- a non-escaping block never requires a heap closure;
- a non-escaping block never requires runtime type erasure;
- a non-escaping block target is statically known;
- a non-escaping block may lower to direct duplicated control flow or to a private direct-call helper;
- release optimization should specialize and inline small block consumers aggressively;
- debug mode may preserve a private direct call for debuggability;
- indirect dispatch is forbidden unless the source explicitly uses a proc or interface value.

For generic block APIs such as `map`, `each`, `filter`, and `times`, prefer monomorphization or equivalent specialization.

Example:

```pop
3.times do |index|
    print(index)
end
```

Should lower semantically to an ordinary integer loop with no iterator allocation and no callable object.

The design must cover:

- code-size limits;
- recursion through block-taking functions;
- debug builds;
- separate compilation;
- cross-module specialization;
- incremental compilation;
- address-taken functions;
- effectful blocks;
- panic/trap paths;
- GC safe points;
- borrow lifetimes;
- exception or typed failure propagation.

A block API must not inhibit:

- induction-variable analysis;
- bounds-check elimination;
- loop-invariant code motion;
- vectorization;
- scalar replacement;
- escape analysis;
- dead-code elimination;
- constant propagation.

HIR should preserve enough structure to reason about block calls, while MIR should contain ordinary explicit blocks, branches, calls, and values.

---

## 9. Collection APIs

Design a standard block-based collection vocabulary that feels idiomatic in Pop.

Examples:

```pop
values.each do |value|
    print(value)
end

local doubled = values.map do |value|
    value * 2
end

local positive = values.select do |value|
    value > 0
end

local total = values.reduce(0) do |sum, value|
    sum + value
end

local first = values.find do |value|
    value.isReady()
end
```

Requirements:

- exact block arity;
- exact input and output types;
- no dynamic unions;
- no hidden iterator object when direct iteration is available;
- no allocation for `each`;
- predictable allocation for materializing operations such as `map`;
- optional lazy views should be a distinct type and API;
- mutation during traversal must follow explicit collection rules;
- block effects participate in the caller's effect analysis;
- array loops should remain friendly to bounds-check elimination and vectorization.

Consider concise receiver shorthand only when unambiguous:

```pop
local names = users.map { |user| user.name }
```

A Crystal-like shorthand such as `&.name` may be considered, but only if it remains statically obvious and grammatically simple.

A Pop-native alternative may be clearer:

```pop
local names = users.map(.name)
```

However, this must not be confused with enum or member shorthand.

The proposal should compare alternatives and reject clever syntax that saves little while increasing parser or reader complexity.

---

## 10. Builder validation and typestate

DSLs should be able to enforce construction rules without dynamic runtime hashes.

Explore typed builders and optional typestate.

Example goal:

```pop
local server = server() do
    .listen(8080)
    .tls do
        .certificate("server.crt")
        .privateKey("server.key")
    end
end
```

Potential compile-time rules:

- `listen` must be specified exactly once;
- a TLS private key requires a certificate;
- mutually exclusive options cannot coexist;
- a route must define a handler;
- a build target must contain at least one source;
- configuration members are unavailable in the wrong nested receiver.

Do not hard-code these rules into the compiler.

They should be expressible through:

- distinct builder types;
- interfaces;
- generics;
- typestate parameters;
- consumed or moved builder states;
- sealed construction phases;
- ordinary compile-time evaluation where appropriate.

The design should avoid exploding user-facing generic types in diagnostics. Library authors may use typestate internally while exposing readable errors through compiler-recognized diagnostic annotations or standard validation interfaces.

Where compile-time enforcement would cause unreasonable complexity, permit deterministic construction-time validation with typed errors. Clearly separate compile-time guarantees from runtime validation.

---

## 11. Effects and capabilities

Prepare the design for an effect-aware Pop without requiring a complete effect system in the first implementation.

A block contract may eventually express effects:

```pop
function withFile(
    path: String,
    mode: FileMode,
    &block: (File) -> Void effects[Io]
): Result[Void, FileError]
```

A pure caller must not accidentally invoke an effectful block.

Scoped receivers may also act as capabilities:

```pop
sandbox do
    .readFile("input.txt")
end
```

Only operations exposed by the scoped receiver are available through receiver shorthand.

This creates a safe DSL boundary:

- no ambient dynamic namespace;
- no reflective access;
- no accidental global capability;
- no hidden privilege escalation;
- capability use remains visible in the type system and lowered calls.

---

## 12. Ownership, borrowing, and capture

Specify capture rules for non-escaping blocks.

Preferred model:

- immutable local capture is allowed by borrow;
- mutable local capture requires explicit mutable access under ordinary Pop rules;
- moved values can be moved into a block only when the call contract allows consumption;
- references obtained from a scoped receiver cannot escape its lifetime;
- the compiler knows the block cannot outlive the call;
- a block may return a borrowed value only when the return lifetime is valid;
- converting captured code into a proc requires explicit syntax and lifetime checking.

Example:

```pop
local total = 0

values.each do |value|
    total = total + value
end
```

This should be legal when `total` is mutable according to Pop's local rules and the traversal is sequential.

Parallel APIs must request stronger contracts:

```pop
values.parallelEach do |value|
    process(value)
end
```

The block may need to be `Send`, non-mutating, isolated, or explicitly synchronized.

Do not give all blocks implicit thread safety.

---

## 13. Control flow

Avoid Ruby and Crystal's most surprising non-local control-flow behavior.

Define:

- `return` returns from the nearest named function;
- the last expression is the normal block result;
- `next value` exits the current block invocation;
- `break` exits loops only;
- `continue` continues loops only;
- a block cannot silently terminate the block-taking function;
- APIs needing early termination use a typed protocol.

Example:

```pop
enum Visit
    Continue
    Stop
end

walk(tree) do |node|
    if node.matches(target) then
        Visit.Stop
    else
        Visit.Continue
    end
end
```

A future opt-in control block may be considered:

```pop
function search(&block: control (Item) -> SearchDecision): Item?
```

But the MVP should not introduce non-local jumps across arbitrary call frames.

Explain how `defer`, resource cleanup, typed failure propagation, and traps behave when a block exits with `next`, `return`, or failure.

---

## 14. Overload and resolution rules

A function with a block and a function without a block may be separate overloads only when resolution remains deterministic.

Example:

```pop
function read(path: String): String
function read(path: String, &consumer: (String) -> Void): Void
```

Rules:

- the presence of a trailing block participates in overload resolution;
- block parameter and return types participate normally;
- the compiler must not type-check arbitrary numbers of overload bodies speculatively in ways that destabilize diagnostics;
- generic block inference must have deterministic limits;
- receiver shorthand resolution occurs only after the block overload is selected;
- no runtime overload selection.

Explain ambiguity diagnostics and formatter behavior.

---

## 15. Generic and interface interaction

Blocks must work with generic functions:

```pop
function map[T, U](values: Array[T], &transform: (T) -> U): Array[U]
```

The compiler should infer `U` from the block result.

Interfaces may declare block-taking methods, but block dispatch itself must not become dynamic.

If an interface method is dynamically dispatched, the receiver call may be indirect, while the trailing block remains a statically known non-escaping argument specialized through an implementation strategy chosen by the compiler.

Discuss realistic lowering strategies:

- interface thunk plus direct block helper;
- generic witness specialization;
- code generation per implementation;
- fallback direct environment pointer without heap allocation;
- restrictions needed for ABI stability.

The public ABI must distinguish block-only entry points from proc-taking entry points.

---

## 16. Diagnostics

Design concrete diagnostics for common mistakes.

Examples:

### Escaping a block

```text
POPxxxx: non-escaping block `handler` cannot be stored in `globalHandlers`
note: block parameters exist only for the duration of this call
help: change the parameter type to an explicit proc if the handler must escape
```

### Wrong arity

```text
POPxxxx: block expects 2 parameters but 1 was declared
note: yielded parameter 2 has type `String`
help: add a second parameter or explicitly ignore it with `_`
```

### Wrong result

```text
POPxxxx: block must return `Int`, but this branch returns `String`
```

### Invalid scoped receiver member

```text
POPxxxx: `timeout` is not available on scoped receiver `RouteBuilder`
note: the nearest scoped receivers are `RouteBuilder` and `HttpBuilder`
help: use `.requestTimeout(...)` on `HttpBuilder` or bind the outer receiver explicitly
```

### Ambiguous receiver nesting

Avoid silent lookup. Require an explicit receiver when more than one scoped receiver could satisfy a shorthand call.

### Hidden allocation

If a programmer expects a block but passes a proc:

```text
POPxxxx: this call uses an escaping proc and may allocate
note: overload `each(&block)` accepts a non-escaping zero-cost block
```

This may be a warning or informational optimization diagnostic, not necessarily an error.

---

## 17. HIR and MIR model

Propose a clean lowering strategy.

### Source

```pop
values.each do |value|
    consume(value)
end
```

### HIR concept

HIR may retain:

- resolved callee;
- block parameter identity;
- exact argument and result types;
- captures;
- scoped receiver type;
- effects;
- escape classification;
- control-flow permissions;
- source spans;
- specialization identity.

Receiver shorthand is already erased:

```text
block block#0(value: Int) -> Void
    call consume(value)
call Array.each[Int](values, block#0)
```

### MIR concept

MIR should not contain Ruby-style blocks.

After specialization it should contain ordinary control flow:

```text
b0:
    index = 0
    length = array.length
    branch b1(index)

b1(index):
    condition = index < length
    condBranch condition b2 b3

b2:
    value = array.getUncheckedAfterProof(index)
    call consume(value)
    next = checkedAdd(index, 1)
    branch b1(next)

b3:
    return
```

Bounds checks may only become unchecked after proof.

For non-inlined debug lowering, MIR may contain a private direct call with an explicit environment structure whose lifetime is stack-bound and non-escaping.

The design must state which guarantees belong to semantics and which belong to optimization.

---

## 18. Standard-library API design principles

Block-based APIs should follow consistent naming.

Prefer clear verbs:

- `each`
- `map`
- `select`
- `reject`
- `find`
- `any`
- `all`
- `reduce`
- `fold`
- `sortBy`
- `groupBy`
- `withLock`
- `withFile`
- `transaction`
- `scope`
- `build`

Avoid APIs whose behavior changes radically depending on block presence unless the overload is obvious.

Do not use a universal `call` protocol as a substitute for specific interfaces.

Do not make every object dynamically callable.

Do not add global monkey patching, open classes, runtime method injection, or implicit conversions merely to support DSL aesthetics.

A Pop DSL should be ordinary statically typed code that reads declaratively.

---

## 19. Non-goals

Explicitly reject the following:

- Ruby-compatible block semantics;
- dynamic `self` replacement;
- `method_missing`;
- runtime name lookup;
- open-ended union inference;
- silent block-to-proc conversion;
- invisible heap closure allocation;
- arbitrary non-local `break`;
- ambiguous brace binding;
- auto-splat based on parameter count;
- magical acceptance of fewer block parameters;
- macros required for basic DSLs;
- compiler-specific DSL modes;
- string-based properties standing in for typed members;
- runtime reflection as the primary DSL mechanism;
- sacrificing deterministic compilation for clever inference.

---

## 20. Suggested MVP

Design an incremental implementation plan.

### Phase 1: Typed trailing blocks

- one trailing block parameter;
- explicit `&block` in the function signature;
- exact arity;
- inferred parameter types;
- typed block result;
- `do ... end` and single-expression `{ ... }`;
- no escaping;
- no first-class conversion;
- no non-local `break`;
- direct invocation and `yield`;
- deterministic overload resolution.

### Phase 2: Collection and resource APIs

- `each`, `map`, `select`, `reduce`;
- `withFile`, locks, transactions;
- capture and borrowing diagnostics;
- HIR specialization;
- no-allocation guarantees;
- optimization conformance tests.

### Phase 3: Scoped receivers

- `scoped Receiver` block contracts;
- leading-dot receiver shorthand;
- nested typed receivers;
- ambiguity diagnostics;
- builder libraries;
- receiver shorthand erased before MIR.

### Phase 4: Explicit procs

- proc literals;
- callable proc types;
- capture layout;
- stack versus heap placement;
- ownership and lifetime rules;
- explicit block/proc API boundaries.

### Phase 5: Typestate and effects

- richer builder validation;
- effect-aware block contracts;
- control protocols;
- concurrency traits;
- optimization reporting.

---

## 21. Required output

Produce a complete language design proposal containing:

1. final recommended syntax;
2. grammar changes;
3. static typing rules;
4. block and proc type representations;
5. overload resolution;
6. capture and lifetime rules;
7. control-flow semantics;
8. scoped receiver rules;
9. effect interaction;
10. HIR representation;
11. MIR lowering;
12. ABI considerations;
13. optimizer guarantees;
14. debug-mode behavior;
15. diagnostics;
16. standard-library examples;
17. rejected alternatives;
18. migration and implementation phases;
19. conformance-test matrix;
20. at least three complete DSL examples.

For every feature, distinguish:

- syntax sugar;
- static semantic rule;
- runtime behavior;
- optimization opportunity;
- optimization guarantee.

Do not justify a feature only because Ruby or Crystal has it.

Every accepted feature must satisfy all of the following:

- it makes Pop code more readable or expressive;
- it remains statically understandable;
- it does not require dynamic lookup;
- it has a clear HIR representation;
- it lowers to ordinary MIR;
- its allocation behavior is predictable;
- its control flow is explicit;
- diagnostics can explain misuse precisely;
- it remains useful outside DSLs;
- it does not weaken Pop's type or safety model.

The final design should make code such as the following feel idiomatic:

```pop
namespace Application

function main()
    local application = webApplication("example") do
        .http do
            .listen("127.0.0.1", 8080)

            .route(.get, "/users/:id") do
                .handle do |request|
                    local id = request.parameter[Int]("id")
                    return Response.json(loadUser(id))
                end
            end
        end
    end

    application.run()
end
```

But it should compile as if the programmer had written explicit typed builder calls and ordinary control flow by hand.

The core principle is:

> Pop DSLs should gain Ruby's readability and Crystal's zero-cost blocks without inheriting their dynamic ambiguity. The syntax may be elegant, but the compiler must always know exactly which value, member, block, effect, lifetime, and control-flow edge it represents.
