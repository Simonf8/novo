<p align="center">
  <img src="assets/logo.png" alt="Novo logo" width="200" />
</p>

<h1 align="center">Novo</h1>

<p align="center"><em>A statically typed language that reads like Python, built to run like Rust.</em></p>

---

```novo
class Point:
    x: Float
    y: Float

    function length() -> Float:
        return (self.x ** 2 + self.y ** 2) ** 0.5

points = [Point(3, 4), Point(6, 8), Point(1, 1)]
far = points where it.length() > 2
show far select f"{it.length():.1f}"      # [5.0, 10.0]
```

Novo is statically typed with full type inference: no annotations are
required, every program is checked before it runs, and errors point at the
exact source with a suggested fix. Functions are generic by default.

Functions that work on numbers compile to machine code with
[Cranelift](https://cranelift.dev) the first time they are called;
everything else runs on the interpreter.

| Benchmark (`benches/`) | Rust `-O` | Novo | CPython 3.13 |
|------------------------|----------:|-----:|-------------:|
| `fib(32)`              | 0.013 s   | 0.026 s | 0.322 s |
| nested integer loops   | 0.100 s   | 0.123 s | 1.585 s |
| Mandelbrot 600×600     | 0.090 s   | 0.101 s | 3.445 s |

> **Status:** early development. Native code currently covers functions
> whose values are all `Int`, `Float`, `Bool` or `None`; lists, strings and
> classes are next. See the [roadmap](#roadmap).

## Building

Requires Rust 1.96 or newer.

```bash
cargo build --release
./target/release/novo run examples/linear_regression.nv
```

Optional features:

```bash
cargo build --release --features python   # import Python modules
cargo build --release --features cuda     # run tensors on NVIDIA GPUs
cargo build --release --features metal    # run tensors on Apple GPUs
```

## Usage

```text
novo run <file.nv> [args...]     Check and run a program
         [--interp]              (without compiling anything to native code)
novo check <file.nv> [--json]    Type-check without running
novo test [path]                 Run test_*.nv and *_test.nv files
novo bench <file.nv> [-n N]      Time repeated runs of a program
novo new <name>                  Create a project
```

`novo check --json` prints one JSON object per diagnostic, for editors.

## A short tour

```novo
# Variables take the type of their first value.
count = 0
names = ["Ada", "Grace"]
ages = {"Ada": 36, "Grace": 45}

# f-strings support Python format specs.
show f"{len(names)} people, average age {ages.values() average:.1f}"

# Control flow
for name in names:
    if ages[name] > 40:
        show name, "is over 40"
    otherwise:
        show name

repeat 3 times:
    count += it            # `it` is the loop index

# Functions are generic; closures capture by value.
function apply_twice(f, x) => f(f(x))
show apply_twice(|n| n * 2, 5)          # 20
show apply_twice(|s| s + "!", "hi")     # hi!!

# Queries over lists
evens = range(10) where it % 2 == 0
show evens select it * it               # [0, 4, 16, 36, 64]
show evens sum                          # 20

# Errors
try:
    show 10 // 0
catch e:
    show "caught:", e                   # caught: division by zero

# Concurrency
task square(n) => n * n
show run [square(2), square(3)] in parallel     # [4, 9]
handle = together square(12)
show await handle                               # 144
```

Tensors and training come from the `nn` module:

```novo
use nn

nn.seed(7)
x = nn.randn([100, 1])
y = x * 3.0 + 2.0

w = nn.param(nn.zeros([1, 1]))
b = nn.param(nn.zeros([1]))
optimizer = nn.sgd([w, b], 0.1)

repeat 200 times:
    loss = nn.mse(x @ w + b, y)
    optimizer.step(loss)        # backward pass and update

show f"w = {w.item():.3f}, b = {b.item():.3f}"     # w = 3.000, b = 2.000
```

[examples/xor_mlp.nv](examples/xor_mlp.nv) trains a small neural network
written as a Novo class, and [examples/tiny_gpt.nv](examples/tiny_gpt.nv) is a
character-level GPT (embeddings, causal multi-head self-attention, layer norm,
MLP blocks) that trains from scratch and generates text in a few seconds.

Modules: `use "path/to/file.nv" as m` loads another file; `use math`,
`use json`, `use time`, `use fs`, `use env`, `use random` and `use nn` bring in
the standard library;
`use python numpy as np` imports a Python module when built with the
`python` feature.

The precise rules (numbers, scoping, classes, error handling, display) are in
[docs/DESIGN.md](docs/DESIGN.md).

## How it works

```text
                                                          ┌──► interpreter   (src/eval)
source ──► lexer/parser ──► type checker ──► typed HIR ───┤
           (src/syntax)     (src/check)                   └──► native code   (src/native)
```

- **`src/syntax`**: an indentation-aware lexer and a recursive-descent parser
  that keep a source span on every node.
- **`src/check`**: name resolution and Hindley–Milner style inference with
  let-polymorphism. Operations on types that are not known yet become
  deferred constraints that travel with a generic function and are checked
  at each use. `Any` is a gradual escape hatch for JSON and Python values.
- **`src/eval`**: the reference interpreter over the typed HIR. Integer
  arithmetic is overflow-checked, maps keep insertion order, and
  `together` and parallel queries run on OS threads.
- **`src/native`**: a JIT built on Cranelift. Each call site records the
  concrete types it instantiates a generic function with, so `fib(n)` called
  with an `Int` compiles to an `Int`-only function. Compiled code checks
  overflow, division by zero and recursion depth, and reports errors exactly
  as the interpreter does. Set `NOVO_NATIVE_LOG=1` to see what compiles and
  why anything does not.
- **`src/diagnostic.rs`**: error rendering with source snippets, labels and
  help text, plus JSON output.

## Testing

```bash
cargo test
```

`tests/programs` holds golden programs: each `.nv` file runs both natively
and with `--interp`, and both outputs are compared with the matching `.out`
file. Leading comments mark expected
errors (`# expect-error: ...`) and required features
(`# requires: python`). After an intentional output change, regenerate with
`NOVO_BLESS=1 cargo test`.

## Roadmap

1. **More native code.** Lists, strings, classes and closures in compiled
   functions, cheaper calls, and ahead-of-time executables.
2. **Shapes in types.** Catch tensor dimension mismatches at check time.
3. **Data and models.** Data loading, batching, reusable layers, and
   running pretrained models such as GPT-2 from their published weights.
4. **Tooling.** Language server, REPL, and a comment-preserving formatter.

## License

Not yet chosen.
