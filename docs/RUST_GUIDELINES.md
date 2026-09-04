# General Rust Project Guidelines

Write idiomatic, safe, robust, maintainable, and efficient Rust.

The goal is not merely to produce code that compiles. Use Rust's type system, ownership model, borrowing rules, algebraic data types, traits, generics, iterator model, error handling, concurrency guarantees, and zero-cost abstractions to make incorrect states and incorrect operations difficult or impossible to express.

Do not mechanically translate C++, Go, Python, Java, or C-style designs into Rust syntax. Reconsider the ownership model, data representation, API boundaries, error model, concurrency model, and abstractions from a Rust-native perspective before implementing them.

## 1. General Language Policy

Use stable Rust and the current project edition unless the repository explicitly specifies otherwise.

Prefer idiomatic Rust over language-neutral or C-style implementations.

Prefer compile-time correctness over runtime conventions whenever practical.

Prefer simple, explicit designs over speculative architecture.

Use advanced Rust features when they improve correctness, abstraction, performance, or maintainability, but do not introduce complexity merely to make the code appear sophisticated.

Do not optimize for minimum line count. Optimize for clarity, correctness, explicit invariants, maintainability, and predictable performance.

## 2. Let Rust Enforce Correctness

Use the compiler and type system as design tools, not obstacles.

Whenever practical:

- encode invariants in types;
- represent state using enums rather than loosely related flags;
- distinguish domain concepts with newtypes;
- use `Option<T>` for legitimate absence;
- use `Result<T, E>` for recoverable failures;
- use ownership to express responsibility;
- use borrowing to express temporary access;
- use mutability only where mutation is required;
- use exhaustive pattern matching to ensure new states are handled;
- use `Send` and `Sync` guarantees rather than bypassing them.

Prefer making invalid states unrepresentable.

For example, prefer:

```rust
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected { session: Session },
}
```

over loosely related fields such as:

```rust
struct Connection {
    is_connected: bool,
    is_connecting: bool,
    session: Option<Session>,
}
```

when those fields allow contradictory states.

Prefer domain types:

```rust
struct UserId(u64);
struct RequestId(u64);
struct Port(u16);
```

when primitive values have different meanings and should not be interchangeable.

## 3. Do Not Translate Other Languages Literally

Do not reproduce C++ ownership patterns, Go interface-heavy patterns, Java object hierarchies, or Python dynamic patterns unless they are genuinely appropriate.

Avoid common translation artifacts such as:

- integer error codes;
- sentinel values such as `-1`;
- boolean fields representing mutually exclusive states;
- pervasive mutable state;
- global mutable singletons;
- getter/setter boilerplate;
- class-style inheritance designs;
- unnecessary heap allocation;
- pointer-like wrappers everywhere;
- defensive copying merely to simplify ownership;
- callback interfaces where enums, generics, iterators, futures, or channels are more natural;
- large interfaces/traits created only to imitate interfaces from another language.

Re-design the solution using Rust concepts first.

## 4. Ownership First

Before implementing behavior, determine who owns each piece of data and how data moves through the system.

Prefer borrowing when ownership transfer is unnecessary.

For read-only inputs, generally prefer:

```rust
&str
&[T]
&Path
```

over:

```rust
&String
&Vec<T>
&PathBuf
```

Use owned types such as:

```rust
String
Vec<T>
PathBuf
```

when the callee needs ownership, needs to store the value, or creates the value itself.

Return owned values when the function creates new data.

Do not force borrowed return values merely to avoid allocation if ownership naturally belongs to the caller.

Keep borrow scopes as short as practical.

When borrow-checker errors occur, reconsider:

1. ownership;
2. data layout;
3. borrowing;
4. scopes;
5. API boundaries;

before introducing cloning, reference counting, `'static`, interior mutability, or `unsafe`.

## 5. Cloning Is a Design Decision

Do not insert `.clone()` merely to satisfy the borrow checker.

Every meaningful clone should have a clear ownership justification.

Before cloning, consider:

- borrowing;
- moving ownership;
- changing the order of operations;
- shortening a borrow;
- restructuring data;
- returning ownership;
- sharing immutable state.

Cloning inexpensive value types is fine when semantically appropriate.

Avoid accidental cloning of large strings, vectors, maps, buffers, tensors, trees, or other expensive structures.

Do not derive `Clone` reflexively on large domain objects merely for convenience.

## 6. Immutability by Default

Use immutable bindings by default.

Prefer:

```rust
let value = ...;
```

and introduce:

```rust
let mut value = ...;
```

only when mutation is actually required.

Keep mutable scopes narrow.

Prefer transformations from one valid state to another rather than exposing long-lived mutable objects that are modified throughout large functions.

Do not use global mutable state unless there is a strong architectural reason.

Avoid `static mut`.

Use safe mechanisms such as `OnceLock`, `LazyLock`, atomics, `Mutex`, or `RwLock` when global/shared state is genuinely required.

## 7. Type Inference

Do not explicitly annotate every local variable.

Use Rust's type inference when the type is obvious from the expression or surrounding context.

Prefer:

```rust
let timeout = Duration::from_secs(30);
let name = String::from("Alice");
let count = items.len();
```

over redundant annotations.

Use explicit local types when they:

- resolve ambiguity;
- specify numeric representation;
- constrain inference intentionally;
- express important domain semantics;
- make a complex expression substantially clearer.

Public API boundaries and struct fields should have deliberate and meaningful types.

Do not add unnecessary explicit lifetime parameters when lifetime elision expresses the same relationship clearly.

Do not introduce `'static` as a generic workaround for lifetime errors.

## 8. Constants

Use `const` for genuine program-level compile-time constants.

Examples:

```rust
const MAX_RETRIES: usize = 5;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
```

Do not replace ordinary immutable local bindings with `const` merely because the expression can be evaluated at compile time.

Prefer ordinary immutable `let` bindings for ordinary local values.

Prefer `const` over `static` when a unique global storage location is unnecessary.

Avoid mutable statics.

## 9. Use Rust's Abstractions

Prefer idiomatic Rust abstractions over manually reproducing lower-level mechanisms.

Use where appropriate:

- enums;
- pattern matching;
- generics;
- traits;
- associated types;
- iterators;
- closures;
- iterator adapters;
- `Option`;
- `Result`;
- newtypes;
- smart pointers;
- RAII;
- ownership transfer;
- borrowing;
- channels;
- futures;
- async/await;
- standard conversion traits;
- standard collection APIs.

Do not rewrite clean iterator pipelines as index-based loops solely because loops appear lower-level.

Do not avoid high-level Rust constructs based on assumed performance costs.

Rust deliberately strives for zero-cost abstractions: high-level abstractions should generally compile to implementations comparable to carefully written lower-level code.

However, do not assume an abstraction is literally free in every context. Measure performance-sensitive code.

## 10. Generics and Static Polymorphism

Use generics actively when an algorithm or data structure is genuinely parameterized over types or behavior.

Prefer static polymorphism when runtime polymorphism is unnecessary.

For example:

```rust
fn process<R: Read>(reader: R) -> Result<Output, Error>
```

or:

```rust
fn process(reader: impl Read) -> Result<Output, Error>
```

may be preferable to unnecessary dynamic dispatch.

Use trait bounds to express the minimum capabilities required by a generic implementation.

Do not add generic parameters merely because Rust supports them.

Do not turn concrete code into:

```rust
Foo<T, U, V, E, C, S>
```

unless those parameters represent real variation.

Remember that monomorphized generics can increase:

- compile time;
- binary size;
- code generation work.

Use `dyn Trait` when runtime polymorphism is genuinely useful, such as:

- heterogeneous collections;
- plugin systems;
- runtime-selected implementations;
- abstraction boundaries where binary-size or compilation tradeoffs favor dynamic dispatch.

Do not treat dynamic dispatch as inherently bad.

Choose static versus dynamic dispatch intentionally.

## 11. Prefer Standard Traits

Use standard Rust traits and ecosystem conventions rather than creating custom equivalents.

Use or derive traits such as:

```rust
Debug
Clone
Copy
PartialEq
Eq
Ord
Hash
Default
Display
From
TryFrom
AsRef
Borrow
Iterator
IntoIterator
```

when their semantics genuinely apply.

Do not implement or derive traits reflexively.

Prefer standard conversion patterns:

```rust
From
Into
TryFrom
TryInto
AsRef
```

over custom conversion functions when applicable.

Use established method naming conventions:

```text
new
with_*
from_*
try_from_*
as_*
to_*
into_*
iter
iter_mut
into_iter
```

## 12. Naming

Follow standard Rust naming conventions.

Use:

```text
snake_case
```

for functions, methods, variables, and modules.

Use:

```text
UpperCamelCase
```

for structs, enums, traits, type aliases, and enum variants.

Use:

```text
SCREAMING_SNAKE_CASE
```

for constants and statics.

Treat acronyms as normal words:

```rust
HttpClient
TcpStream
Uuid
```

rather than:

```rust
HTTPClient
TCPStream
UUID
```

Use descriptive names based on domain meaning.

Short names such as `i`, `j`, `x`, `tx`, `rx`, and `T` are acceptable where their meaning is conventional and their scope is small.

Use conventional generic names such as:

```text
T
U
E
K
V
R
W
F
```

for simple generic roles.

For complex generic APIs, use descriptive generic type names when they improve readability.

Do not use Java-style getter naming unnecessarily.

Prefer:

```rust
fn name(&self) -> &str
fn name_mut(&mut self) -> &mut String
```

rather than:

```rust
fn get_name(&self) -> &str
```

## 13. Error Handling

Use `Option<T>` for legitimate absence.

Use `Result<T, E>` for recoverable failures.

Do not use sentinel values or magic numeric error codes.

Use `?` for normal error propagation.

External failures such as:

- invalid input;
- file-system failures;
- network failures;
- timeouts;
- parsing errors;
- serialization errors;
- database errors;
- unavailable resources;

must normally be represented as recoverable errors rather than panics.

Reserve `panic!`, `unreachable!`, `unwrap()`, and similar operations primarily for:

- programmer bugs;
- violated internal invariants;
- tests;
- examples;
- deliberately fatal initialization;
- states that have been logically proven impossible.

Do not scatter `unwrap()` throughout production code.

When failure is genuinely impossible because of an invariant, prefer a meaningful `expect(...)` message explaining that invariant over an unexplained `unwrap()`.

For reusable libraries, prefer structured typed errors that callers can inspect.

For applications, contextual error aggregation such as `anyhow` may be appropriate.

Do not expose internal implementation errors unnecessarily through public APIs.

## 14. Control Flow

Prefer clear, shallow control flow.

Use:

- `?`;
- early returns;
- `let ... else`;
- exhaustive `match`;

when they improve readability.

Avoid deeply nested `if let` / `match` structures when the same logic can be expressed as validation followed by the main path.

Prefer:

```rust
let Some(value) = value else {
    return Ok(());
};
```

over unnecessary nesting.

Use exhaustive enum matches when handling all states matters.

Avoid `_ => ...` when explicitly enumerating variants would allow the compiler to detect newly introduced states.

## 15. Functions and Modules

Each function should represent one coherent operation.

Do not impose arbitrary line-count limits.

Split functions when doing so improves:

- semantic cohesion;
- invariant management;
- readability;
- testability;
- reuse.

Prefer pipelines such as:

```rust
let request = parse_request(input)?;
let request = validate_request(request)?;
let command = build_command(request)?;
let response = execute(command).await?;
```

when the stages represent genuine semantic boundaries.

Organize modules around domain concepts and responsibilities rather than creating excessively deep directory hierarchies or one-file-per-type structures.

Keep public APIs intentionally small.

Prefer private implementation details by default.

Expose only what callers need.

## 16. Collections and Allocation

Avoid unnecessary allocation and copying.

Before allocating a new collection, determine whether the operation can consume or iterate over an existing collection directly.

Avoid unnecessary intermediate `Vec`s created only to iterate immediately afterward.

Prefer lazy iterator processing when it remains clear.

Use `Vec::with_capacity`, `String::with_capacity`, and similar facilities when expected capacity is reliably known or cheaply estimated.

Do not invent arbitrary capacity values without justification.

Avoid excessive temporary `String` creation.

Prefer borrowed string slices when ownership is unnecessary.

Do not pursue zero-allocation or zero-copy designs at the expense of major complexity unless profiling demonstrates meaningful benefit.

## 17. Iterators

Use iterators and iterator adapters naturally.

Prefer operations such as:

```text
map
filter
filter_map
find
any
all
fold
collect
zip
enumerate
flat_map
```

when they clearly express the operation.

Do not force every loop into an iterator chain.

A straightforward `for` loop is preferable when it is clearer.

Avoid manual indexing when iteration naturally expresses the operation.

Manual indexing is appropriate when the algorithm genuinely depends on indices.

## 18. Numeric Safety

Avoid unchecked lossy casts when representability matters.

Prefer:

```rust
u32::try_from(value)?
value.try_into()?
```

over:

```rust
value as u32
```

when truncation would be a bug.

Use explicitly chosen arithmetic semantics where overflow matters:

```text
checked_*
saturating_*
wrapping_*
overflowing_*
```

Do not rely accidentally on debug/release overflow differences.

Avoid magic numbers.

Use named constants or domain types when values have semantic meaning.

## 19. Shared State

Do not introduce:

```rust
Arc<Mutex<T>>
Rc<RefCell<T>>
Arc<RwLock<T>>
```

merely because they make ownership errors disappear.

Before introducing shared mutable ownership, consider:

1. single ownership;
2. moving ownership;
3. borrowing;
4. immutable shared ownership;
5. state partitioning;
6. message passing.

Use the simplest ownership model that correctly expresses the architecture.

Use `Rc` for single-threaded shared ownership.

Use `Arc` when shared ownership must cross threads/tasks.

Add synchronization only when mutation or coordination is genuinely required.

## 20. Concurrency

Use Rust's ownership, `Send`, and `Sync` properties as part of the concurrency design.

Never introduce:

```rust
unsafe impl Send for ...
unsafe impl Sync for ...
```

merely to satisfy compiler errors.

Such implementations require explicit soundness justification and careful review.

Minimize shared mutable state.

Prefer message passing or partitioned ownership when it produces a simpler concurrency model.

Keep lock scopes short.

Do not perform unrelated expensive work while holding locks.

Be aware of lock ordering and potential deadlocks.

## 21. Async Rust

Do not treat async Rust as synchronous Rust with `.await` added.

Avoid blocking executor threads.

Do not perform long blocking I/O or CPU-heavy operations directly in async tasks.

Use appropriate blocking pools, worker threads, dedicated CPU executors, or `spawn_blocking` when necessary.

Avoid holding synchronous lock guards across `.await`.

Use bounded concurrency.

Prefer:

- bounded channels;
- semaphores;
- queue limits;
- backpressure;
- cancellation;
- timeouts;
- explicit concurrency limits.

Avoid:

- unbounded task spawning;
- unbounded queues;
- unbounded retries;
- uncontrolled fan-out.

Design cancellation and shutdown behavior intentionally in long-running services.

## 22. Resource Safety

Memory safety alone does not guarantee operational safety.

For services and systems code, consider explicit limits for:

- request size;
- response size;
- queue depth;
- concurrency;
- memory usage;
- retry count;
- connection count;
- timeouts;
- task count;
- file descriptors;
- recursion depth where relevant.

Never assume that because code is Safe Rust it cannot experience:

- deadlocks;
- livelocks;
- denial of service;
- out-of-memory conditions;
- infinite loops;
- starvation;
- resource exhaustion;
- logical races.

## 23. Unsafe Rust

Prefer Safe Rust.

Do not introduce `unsafe` merely to:

- bypass the borrow checker;
- avoid redesigning ownership;
- make compilation succeed;
- perform speculative optimization;
- imitate C/C++ implementation patterns.

For projects that do not require unsafe code, consider:

```rust
#![forbid(unsafe_code)]
```

If unsafe code is genuinely required:

1. keep the unsafe region minimal;
2. isolate it behind a safe abstraction;
3. minimize the exposed unsafe API;
4. document the required invariants;
5. document why each invariant holds;
6. test boundary cases thoroughly;
7. explicitly delimit unsafe operations;
8. review unsafe code separately from ordinary code.

Every unsafe block should have a meaningful `SAFETY:` explanation.

Example:

```rust
// SAFETY:
// `index < slice.len()` was checked above, so `index` points to
// an initialized element within the allocation.
unsafe {
    slice.get_unchecked(index)
}
```

Never write meaningless comments such as:

```rust
// SAFETY: this is safe.
```

For crates that permit unsafe code, prefer explicit unsafe blocks even inside unsafe functions.

Do not use unsafe solely for hypothetical performance improvements.

Require benchmark/profile evidence before replacing clear Safe Rust with a more complex unsafe implementation.

## 24. Zero-Cost Abstractions

Prefer high-level Rust abstractions when they preserve clarity and correctness.

Do not manually decompose abstractions merely because lower-level code looks faster.

Rust abstractions such as:

- generics;
- iterators;
- closures;
- enums;
- pattern matching;
- RAII;
- newtypes;
- trait-based static dispatch;

are frequently optimized aggressively and should be used naturally.

Static generic dispatch should generally be preferred over manual function-pointer tables when runtime polymorphism is unnecessary.

Do not assume all abstractions are universally zero-cost.

Potential costs include:

- allocation;
- dynamic dispatch;
- reference counting;
- synchronization;
- monomorphization/code size;
- cache behavior;
- additional copies;
- runtime bounds checks;
- serialization.

When performance is important, inspect optimized Release behavior and benchmark rather than reasoning only from source syntax.

## 25. Performance

Write clear idiomatic Safe Rust first.

Avoid obvious unnecessary:

- clones;
- allocations;
- conversions;
- locking;
- serialization;
- intermediate collections;
- repeated computation;
- dynamic dispatch in hot paths when static dispatch is suitable.

Do not introduce significant implementation complexity based on guessed bottlenecks.

For performance-sensitive changes:

1. establish a baseline;
2. benchmark Release builds;
3. profile;
4. identify the actual bottleneck;
5. optimize that bottleneck;
6. benchmark again;
7. preserve correctness tests.

Do not draw performance conclusions from debug builds.

Prefer algorithmic and architectural improvements over micro-optimization.

## 26. Traits and Abstraction

Introduce traits for real abstraction boundaries.

Good reasons include:

- multiple meaningful implementations;
- generic algorithms;
- dependency inversion at a stable boundary;
- reusable library APIs;
- testing substitution when appropriate.

Do not create traits solely because a type might theoretically have another implementation one day.

Avoid unnecessary trait hierarchies.

Avoid excessive generic bounds.

Use associated types when a trait implementation naturally has one canonical associated type.

Use generic type parameters when callers genuinely choose among multiple types.

Use `impl Trait` when it simplifies an API without hiding important semantics.

Use `dyn Trait` intentionally when runtime polymorphism is appropriate.

## 27. Macros

Prefer functions, generics, traits, and ordinary language constructs before introducing macros.

Use declarative macros when they eliminate genuine repetitive syntax that functions cannot express conveniently.

Use procedural macros only when their benefit justifies the additional build complexity, debugging difficulty, and maintenance burden.

Do not create macros merely to make ordinary code shorter.

Macro-generated APIs should remain understandable through documentation and tooling.

## 28. Dependencies

Prefer the standard library when it solves the problem cleanly.

Use mature ecosystem crates when they substantially reduce complexity or provide well-tested functionality.

Do not add a dependency for trivial functionality without considering its cost.

Before adding a dependency, consider:

- maintenance status;
- API stability;
- security history;
- dependency tree;
- compile-time cost;
- feature flags;
- licensing where relevant;
- whether the standard library already suffices.

Avoid enabling every Cargo feature by default.

Enable only the features the project actually needs when practical.

Do not obsessively minimize features when doing so would meaningfully reduce maintainability.

## 29. Public API Design

Design public APIs deliberately.

Prefer APIs that:

- make ownership behavior clear;
- accept borrowed forms where appropriate;
- use domain-specific types;
- expose structured errors;
- avoid unnecessary implementation details;
- follow Rust ecosystem conventions;
- remain difficult to misuse.

Prefer constructors that establish invariants once.

After validated construction, internal code should operate on valid domain types rather than repeatedly checking primitive inputs.

Consider builders when construction has many optional parameters.

Avoid functions with many positional booleans or unrelated primitive arguments.

Prefer:

```rust
enum Transport {
    Plain,
    Tls,
}
```

over parameters such as:

```rust
connect(host, port, true);
```

## 30. Documentation

Document public APIs with rustdoc where appropriate.

Document:

- semantic behavior;
- important invariants;
- ownership expectations when non-obvious;
- error conditions;
- panic conditions;
- safety requirements;
- concurrency assumptions;
- non-obvious performance characteristics.

Use standard sections where applicable:

```text
# Errors
# Panics
# Safety
# Examples
```

Comments should explain why code exists, not merely restate what the code does.

Prefer:

```rust
// Skip the sentinel entry at index 0.
```

over:

```rust
// Increment the index.
```

## 31. Formatting

Use `rustfmt` as the formatting authority.

Do not manually align assignments, fields, or declarations for visual appearance.

Do not invent custom formatting conventions unless the repository already has them.

Generated code should remain naturally readable after:

```bash
cargo fmt
```

Prefer normal Rust formatting over dense one-line expressions.

## 32. Compiler Warnings

Project-owned Rust code should normally compile without warnings.

Do not ignore compiler warnings.

Do not suppress warnings merely to make CI pass.

Fix the underlying issue whenever practical.

Do not introduce broad crate-level:

```rust
#![allow(...)]
```

directives merely to silence warnings.

If a warning is intentionally allowed:

- scope the allow as narrowly as practical;
- use the exact lint name;
- document the reason when it is not obvious.

Do not modify third-party dependency code merely to eliminate warnings outside the project's control.

## 33. Clippy

Treat Clippy as part of the normal development process.

Generated code should pass:

```bash
cargo clippy --all-targets --all-features
```

without unexplained warnings.

Prefer fixing Clippy findings rather than adding `#[allow(...)]`.

Do not blindly enable every Clippy lint group.

The standard correctness, suspicious, style, complexity, and performance lints should be respected.

`clippy::pedantic` may be selectively used where beneficial.

Do not enable the entire `clippy::restriction` or `clippy::nursery` groups without deliberate project-specific justification.

## 34. Warning-Clean CI

For first-party project code, CI should normally treat warnings as failures.

A typical validation pipeline is:

```bash
cargo fmt --all -- --check

cargo check --all-targets --all-features

cargo clippy --all-targets --all-features -- -D warnings

cargo test --all-features
```

When strict warning policies are used, prefer a stable/pinned Rust toolchain appropriate for the project's compatibility policy so toolchain updates do not unpredictably break CI through newly introduced lints.

Do not lower lint levels solely to make generated code pass.

## 35. Testing

Write tests for behavior, invariants, boundaries, and failure paths rather than only happy-path examples.

Use:

- unit tests for local behavior;
- integration tests for public behavior;
- property-based testing where invariants span large input spaces;
- fuzzing for parsers, protocol handlers, binary formats, unsafe boundaries, and hostile inputs when appropriate.

Test errors as deliberately as success cases.

Regression fixes should normally include regression tests.

Concurrency-sensitive code should include tests for shutdown, cancellation, contention, and error propagation where practical.

Unsafe abstractions require particularly strong testing.

## 36. Security

Treat all external input as untrusted at system boundaries.

Validate and normalize input before converting it into trusted domain types.

Avoid:

- unchecked indexing of untrusted data;
- unchecked integer conversion;
- unbounded allocation based on attacker-controlled sizes;
- unbounded decompression;
- unbounded recursion;
- unbounded task creation;
- uncontrolled retry amplification;
- path traversal;
- command injection;
- accidental secret logging.

Do not rely on memory safety alone as a complete security model.

Keep secrets out of logs and diagnostic output.

Use well-established cryptographic crates rather than implementing cryptographic primitives manually.

## 37. Logging and Observability

Use structured logging/tracing for nontrivial services.

Prefer stable semantic fields over constructing large formatted strings.

Do not log sensitive values unnecessarily.

Distinguish normal expected failures from operationally significant errors.

Avoid excessive logging in hot loops.

Add metrics around important resource and performance boundaries when appropriate.

## 38. AI-Generated Rust Review Checklist

When generating or reviewing Rust code, explicitly inspect every occurrence or unnecessary proliferation of:

```text
.clone()
unwrap()
expect()
panic!()
unsafe
'static
Arc
Mutex
RwLock
RefCell
Box<dyn Trait>
as casts
collect()
spawn()
unbounded channels
#[allow(...)]
```

These constructs are not inherently wrong.

Their presence should trigger the question:

"Is this construct semantically necessary, or was it introduced merely to bypass Rust's constraints or simplify code generation?"

Pay particular attention to these AI failure modes:

- cloning instead of solving ownership correctly;
- using `Arc<Mutex<_>>` as a universal ownership solution;
- adding `'static` to silence lifetime errors;
- introducing `unsafe` to bypass borrowing rules;
- using `unwrap()` instead of designing errors;
- adding unnecessary generic abstraction;
- generating Java/C++-style APIs;
- allocating intermediate collections unnecessarily;
- enabling large dependency feature sets unnecessarily;
- hiding warnings with `#[allow]`;
- holding locks across `.await`;
- spawning unlimited asynchronous tasks;
- introducing runtime polymorphism where static dispatch is simpler;
- manually optimizing code without benchmarks.

## 39. Required Validation Before Completion

Do not consider a Rust implementation complete merely because it appears syntactically correct.

When tooling is available, run:

```bash
cargo fmt

cargo check --all-targets

cargo clippy --all-targets --all-features

cargo test
```

For production/CI validation, prefer:

```bash
cargo fmt --all -- --check

cargo check --all-targets --all-features

cargo clippy --all-targets --all-features -- -D warnings

cargo test --all-features
```

Also run project-specific tests, benchmarks, integration tests, fuzz tests, or security checks when relevant.

Do not claim that code compiles, tests pass, or warnings are clean unless those commands were actually executed successfully.

## 40. Decision Priority

When several implementations are possible, prefer them approximately in this order:

1. Correct and sound.
2. Safe Rust.
3. Clear ownership and lifetime model.
4. Invalid states prevented by types.
5. Idiomatic Rust.
6. Simple and maintainable architecture.
7. Clear error semantics.
8. Minimal unnecessary allocation/copying/synchronization.
9. Appropriate zero-cost abstractions.
10. Measured performance improvements.
11. Unsafe or highly specialized optimization only when justified.

The objective is not to write Rust that merely resembles systems code.

The objective is to write code that takes advantage of the reasons Rust exists: strong compile-time guarantees, explicit ownership, safe concurrency, expressive types, predictable performance, and powerful abstractions without unnecessarily paying runtime costs.