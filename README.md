# Novo — Fast, Intent-Driven Programming Language

Novo is a production-grade, high-performance general-purpose programming language designed to combine Python-level approachability, Go-level practicality, and native execution speed.

Novo lets developers express intent directly with zero boilerplate while executing orders of magnitude faster than CPython.

---

## Key Features

- **Intuitive, Human-Centric Syntax**: No unnecessary boilerplate, natural control flow (`repeat x times`, `otherwise`, `where`, `select`).
- **High-Performance Execution Engines**:
  - **Register/Stack Bytecode Virtual Machine (`--engine=vm`)**: SSA-optimized bytecode execution with specialized opcodes and jump threading.
  - **Dynamic x86_64 JIT Engine (`--engine=jit`)**: Native in-memory x86_64 machine code compilation via direct OS page allocation (`VirtualAlloc` on Windows / `mmap` on Unix).
  - **Tree-Walking Reference Interpreter (`--engine=ast`)**: Baseline semantic reference engine.
  - **Differential Engine (`--engine=diff`)**: Dual-executes AST and VM pipelines and verifies exact semantic and output parity.
- **First-Class Concurrency Engine**: Native OS thread-pool execution via `task`, `run [ ... ] in parallel`, and `together [ ... ]` with lock-free/condvar-synchronized promises.
- **Rich Standard Library**: Zero-dependency standard library covering `math`, `time`, `fs`, `json` (fully native recursive-descent parser and serializer), `env`, and comprehensive collection methods (`str`, `list`, `map`).
- **Modern Developer Toolchain**:
  - `novo run <file.nv>`: Run Novo programs across any execution engine.
  - `novo check <file.nv>`: Static syntax, type checking, and IR generation without execution.
  - `novo test [dir|file]`: Automated test discovery and differential verification.
  - `novo bench <file.nv>`: Statistical microbenchmark engine reporting mean, median, min, max, and std-dev.
  - `novo fmt <file|dir> [-w]`: Canonical AST-driven source code formatter.
  - `novo lint <file|dir>`: Static code analyzer catching unused variables, dead code, and style issues.
  - `novo new <project_name>`: Scaffold a complete Novo project with structure and tests.
  - `novo ir <file.nv>`: Emit optimized SSA intermediate representation and basic blocks.
- **Seamless Python Interoperability**: Direct zero-ceremony access to the Python ecosystem (`numpy`, `pandas`, `cv2`, `torch`) via PyO3 without sacrificing Novo performance.

---

## Installation & Building

Novo is implemented in 100% safe, high-performance Rust.

### Prerequisites
- Rust 1.80+ (`cargo`, `rustc`)
- Python 3.10+ (for Python interop feature)

### Building from Source

```bash
cd novo
cargo build --release
```

The compiled binary will be placed at `target/release/novo` (or `novo.exe` on Windows).

---

## CLI Usage

```text
Usage:
    novo [command] [options] <file.nv>

Commands:
    run <file.nv>              Execute a Novo program (default)
    check <file.nv>            Validate syntax, types, and IR generation
    test [dir|file]            Discover and run test suite with differential verification
    bench <file.nv>            Run repeated microbenchmark measurements
    fmt <file|dir> [-w]        Format Novo source code to canonical style
    lint <file|dir>            Run static analysis and linter for code health
    new <project_name>         Scaffold a new Novo project
    ir <file.nv>               Emit optimized SSA intermediate representation

Options:
    --engine=<vm|ast|diff|jit> Execution engine (default: vm)
    --iterations=<N>, -n <N>   Benchmark iterations (default: 5)
    --write, -w                Write formatted code directly to file (for fmt)
    --help, -h                 Show help message
    --version, -v              Show version information
```

---

## Language Quickstart

### Hello World & Natural Control Flow

```novo
show "Hello, World from Novo!"

count = 3
repeat count times:
    show "Counting..."

score = 85
if score >= 90:
    show "Grade: A"
otherwise if score >= 80:
    show "Grade: B"
otherwise:
    show "Grade: C"
```

### Functions & Closures

```novo
function factorial(n: integer) -> integer:
    if n <= 1:
        return 1
    return n * factorial(n - 1)

show "10! = " + str(factorial(10))
```

### Standard Library (JSON, FS, Math, Time)

```novo
use json
use math
use fs

# Native JSON parsing and stringification (zero Python overhead)
payload = "{\"model\": \"novo-fast\", \"layers\": 12, \"active\": true}"
config = json.parse(payload)
show "Config model: " + config["model"]

# Native Math Primitives
hypot = math.sqrt(math.pow(3.0, 2.0) + math.pow(4.0, 2.0))
show "Hypotenuse: " + str(hypot)

# Filesystem IO
fs.write_file("output.txt", "Engineered with Novo")
content = fs.read_file("output.txt")
show "Read from disk: " + content
```

### First-Class Concurrency

```novo
task worker_a():
    return 100 * 2

task worker_b():
    return 300 + 400

# Runs tasks simultaneously on native thread pool without async/await coloring
results = run [worker_a(), worker_b()] in parallel
show "Total parallel sum: " + str(results[0] + results[1])
```

---

## Testing & Quality Assurance

Novo ships with an integrated test runner and differential execution verifier:

```bash
# Run all tests in differential mode (verifies VM == AST parity)
novo test tests --engine=diff

# Run all tests natively with JIT compilation
novo test tests --engine=jit
```
