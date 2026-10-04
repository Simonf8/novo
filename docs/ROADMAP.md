# Novo — High-Tier Language Engineering Roadmap

## Mission

Evolve Novo from a promising interpreter into a **production-grade, high-performance general-purpose programming language and ecosystem**.

Novo should not become a feature-heavy language for the sake of completeness. Every capability must reinforce the central thesis:

> **Novo lets developers express intent directly, with less code, fewer concepts, and less boilerplate — while executing significantly faster than Python.**

The target is not to build "another Python."

The target is to create a language that combines:

- Python-level approachability
- Go-level practicality
- Rust-level performance potential
- Modern data-processing primitives
- First-class concurrency
- A world-class developer experience
- Extremely low cognitive overhead

The architecture must therefore be designed for long-term scale without making the language itself complicated.

---

# 1. High-Performance Execution Engine

The current AST-walking interpreter is a development milestone, not the final execution architecture.

Move Novo toward:

```text
Source
  ↓
Lexer
  ↓
Parser
  ↓
AST
  ↓
Semantic Analysis
  ↓
Type Inference / Checking
  ↓
Intermediate Representation
  ↓
Bytecode / Native Backend
  ↓
Optimised Execution
```

### Phase 1 — Bytecode VM

Implement a proper bytecode compiler and virtual machine.

The VM should eliminate repeated AST traversal and dramatically reduce:
- dispatch overhead
- dynamic lookup overhead
- expression evaluation overhead
- loop overhead
- function-call overhead

Benchmark the VM against the current interpreter.

### Phase 2 — Optimisation

Introduce evidence-driven optimisation:
- constant folding
- dead-code elimination
- constant propagation
- efficient integer operations
- specialised operations
- inline caching
- efficient local-variable access
- function inlining where appropriate
- escape analysis
- allocation reduction

Do not implement optimisations simply because they sound impressive. Profile first.

### Phase 3 — Native Compilation

Design the IR so Novo can eventually compile directly to native machine code.
Potential backends may include LLVM, Cranelift, or another appropriate technology.

The ultimate goal:
> **Idiomatic Novo should have a credible path to compiled-language performance.**

---

# 2. Modern Type System Without Rust-Level Complexity

Novo should support both inference and explicit types.

Simple code should remain simple:

```novo
name = "Simon"
age = 23
```

Explicit types should be available when useful:

```novo
name: str = "Simon"
age: int = 23
```

Build a type system capable of catching meaningful errors before execution while preserving Novo's simplicity.

The system should support, where justified:
- primitive types
- collections
- functions
- optional/null values
- unions
- generics
- structural/object types
- type inference
- type narrowing

Avoid forcing users into verbose type declarations.

**Principle:**
> Types should help the programmer, not become the programmer's job.

---

# 3. Memory Management

Adopt **automatic generational garbage collection** as Novo's default memory model.

Do NOT copy Rust's ownership/borrowing model into the language.

Novo's users should not need to understand:
- lifetimes
- borrowing
- ownership annotations
- move semantics
- manual deallocation

The runtime should handle ordinary memory automatically.

For external resources such as files, sockets, locks and devices, provide deterministic cleanup:

```novo
with file = open "data.txt":
    text = file read
```

The intended model is:

```text
Ordinary memory
→ Generational GC

External resources
→ Deterministic cleanup

Performance
→ Compiler/runtime optimisation
```

The GC must be designed with future native compilation, concurrency and high-performance workloads in mind.

---

# 4. First-Class Concurrency

Concurrency must be a **language-level advantage**, not a library maze.

Do not reproduce Python's style of programming (`threading`, `asyncio`, `Future`, `Executor`, `EventLoop`, `ThreadPoolExecutor`).

The programmer should describe intent:

```novo
results = urls map fetch together
```

The runtime should decide whether the appropriate implementation is asynchronous I/O, thread pools, work stealing, parallel execution, or task scheduling.

For advanced applications, internally support:
- structured concurrency
- cancellation
- task supervision
- error propagation
- synchronisation
- parallel CPU workloads

But keep the common API extremely small.

### Critical requirement
Do not make concurrency syntax complicated. Novo should make concurrent programming easier than Python.

---

# 5. Standard Library Designed Around Intent

Build a modern standard library around the things developers actually do.

Core areas:
```text
Files
HTTP
JSON
CSV
Dates / Time
Regex
Collections
Processes
Networking
Compression
Cryptography
Environment
CLI
Serialization
Logging
Testing
```

Prefer high-level operations:

```novo
data = read "users.csv" as csv
```

rather than requiring developers to manually construct parsers, readers and decoding pipelines for common operations.
The standard library should eliminate boilerplate without hiding important behaviour.

---

# 6. Data-Oriented Programming as a Core Strength

Novo already demonstrates the value of operations such as:

```novo
users where user.age >= 18
users select user.name
```

Expand this philosophy. Common data transformations should be concise and composable:

```novo
active = users where user.active
names = active select user.name
total = users sum user.salary
average = users average user.salary
```

Support efficient execution so these abstractions are not merely syntactic sugar. The compiler/runtime should be able to optimise pipelines where possible.

Novo should become exceptionally strong for:
- data processing
- sensor data
- robotics
- automation
- APIs
- files
- scientific workloads
- machine learning pipelines

without requiring developers to learn a large framework.

---

# 7. Python Interoperability as a Strategic Advantage

Python compatibility should remain a major Novo advantage.

The goal is:

```novo
use python numpy
use python cv2
use python pandas
```

with minimal friction. Python objects should behave naturally inside Novo.

However, clearly separate:

### Native Novo execution
```text
Novo → Novo runtime
```

### Python interoperability
```text
Novo → Python runtime → Python library
```

Do not claim that Python interoperability is automatically zero-overhead.
Measure:
- conversion overhead
- function-call overhead
- object wrapping
- GIL effects
- startup cost
- data transfer cost

Native Novo code must remain independent from Python when Python is not being used.

Long-term:
> Novo should be able to access the Python ecosystem without depending on Python for Novo's own performance.

---

# 8. Production-Grade Toolchain

A serious language requires a serious toolchain.

The core CLI should eventually support:

```bash
novo new
novo run
novo build
novo test
novo fmt
novo lint
novo check
novo benchmark
novo profile
novo debug
novo package
```

The development workflow should be coherent from project creation to deployment. The developer should not need to assemble a collection of unrelated tools.

---

# 9. World-Class Developer Experience

The language should be supported by a proper language server.

Build:
- LSP
- VS Code extension
- syntax highlighting
- autocomplete
- diagnostics
- go-to-definition
- find references
- rename/refactoring
- formatting
- debugging
- inline errors
- documentation lookup

Error messages should remain one of Novo's defining characteristics.

For example:

```text
Type error: expected Number, received String

  12 | total = price + "5"
     |                ^^^

`price` is a Number.
The value "5" is a String.

Try:
    price + 5
```

The compiler should explain the problem rather than merely report it.

---

# 10. Package Ecosystem, Distribution and Long-Term Platform

A high-tier language is not only a compiler. It needs an ecosystem.

Design `novo.toml` and `novo.lock` with:
- dependency management
- semantic versioning
- reproducible builds
- package publishing
- package discovery
- dependency resolution
- security checks
- project metadata
- build configuration

Eventually establish an official package registry.

The goal should be:

```bash
novo add numpy
novo add opencv
novo add web
novo build
```

producing a reproducible application.
Support cross-platform targets where practical (Windows, Linux, macOS) and eventually additional architectures/platforms as the compiler matures.

---

# Strategic Architecture

The long-term Novo architecture should look approximately like this:

```text
                         NOVO
                           │
                     Source Code
                           │
                    Lexer / Parser
                           │
                          AST
                           │
                 Semantic Analysis
                           │
                 Type Inference/Check
                           │
                          IR
                    ┌──────┴──────┐
                    │             │
                 Bytecode      Native
                    │             │
                    ▼             ▼
                 Fast VM      Machine Code
                    │             │
                    └──────┬──────┘
                           │
                     Novo Runtime
                           │
       ┌───────────────────┼───────────────────┐
       │                   │                   │
   Standard Lib       Concurrency        Python Bridge
       │                   │                   │
       └───────────────────┼───────────────────┘
                           │
                     Package System
                           │
                    Developer Tools
                           │
            ┌──────────────┼──────────────┐
            │              │              │
           LSP           Debugger       Profiler
            │              │              │
         VS Code        Tooling         Optimisation
```

---

# The Golden Rule

Every proposed feature must pass this test:

### Does it reduce developer effort?
### Does it reduce boilerplate?
### Does it reduce cognitive load?
### Does it improve performance?
### Does it make common programs shorter without making them cryptic?

If the answer is no, question whether the feature belongs in Novo.
Do not add features merely because another language has them.
Do not add syntax merely to win benchmarks.
Do not sacrifice simplicity for theoretical completeness.
Do not optimise benchmark code at the expense of real-world performance.

---

# Performance Philosophy

Novo should be evaluated honestly against:
- CPython
- PyPy
- Go
- Rust
- JavaScript/Node.js where relevant

Benchmark categories should include:
- CPU-heavy workloads
- loops
- collections
- file processing
- JSON
- CSV
- networking
- concurrency
- database operations
- image processing
- robotics/sensor workloads
- ML inference

Use repeated measurements and report Mean, Median, Min, Max, Standard deviation, Memory usage, Startup time.

Separate:
- Novo interpreter
- Novo bytecode VM
- Novo native build
- Python interoperability
- External-library execution

Never present Python-library execution as native Novo performance.

---

# What Novo Must NOT Become

Do not allow Novo to drift into:
- Rust-level syntactic complexity
- Java-style boilerplate
- C++-style language complexity
- JavaScript ecosystem fragmentation
- Python's packaging problems
- endless configuration
- framework-driven programming
- syntax added solely for marketing
- benchmark-specific hacks

The language should remain small, coherent and predictable.

---

# The End State

The ideal Novo program should feel almost obvious.

Instead of:
```text
How do I implement this?
Which library do I need?
Which abstraction should I use?
How do I configure concurrency?
How do I manage this resource?
How do I parse this file?
How do I connect these APIs?
```

the developer should be able to express:
```text
This is what I want.
```

and Novo should handle the machinery.

### The ultimate product thesis:

> **Python made programming accessible.**
> **Novo should make programming effortless.**

Build toward that standard.
