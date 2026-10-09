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

> **Status:** early development. The front end, type checker and reference
> interpreter are complete and tested. Native compilation, which is where
> the speed comes from, is the next milestone; see the [roadmap](#roadmap).

## Building

Requires Rust 1.85 or newer.

```bash
cargo build --release
./target/release/novo run examples/linear_regression.nv
```

Python interop is optional, because it links against a Python installation:

```bash
cargo build --release --features python
```

## Usage

```text
novo run <file.nv> [args...]     Check and run a program
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

Modules: `use "path/to/file.nv" as m` loads another file; `use math`,
`use json`, `use time`, `use fs` and `use env` bring in the standard library;
`use python numpy as np` imports a Python module when built with the
`python` feature.

The precise rules (numbers, scoping, classes, error handling, display) are in
[docs/DESIGN.md](docs/DESIGN.md).

## How it works

```text
source ──► lexer ──► parser ──► type checker ──► typed HIR ──► interpreter
           src/syntax           src/check                      src/eval
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
- **`src/diagnostic.rs`**: error rendering with source snippets, labels and
  help text, plus JSON output.

## Testing

```bash
cargo test
```

`tests/programs` holds golden programs: each `.nv` file runs and its output
is compared with the matching `.out` file. Leading comments mark expected
errors (`# expect-error: ...`) and required features
(`# requires: python`). After an intentional output change, regenerate with
`NOVO_BLESS=1 cargo test`.

## Roadmap

1. **Native code generation.** Lower the typed HIR to Cranelift with unboxed
   `Int`, `Float` and `Bool`, and monomorphized generics. The goal is
   numeric code within a small factor of Rust.
2. **Tensors and autograd.** A built-in `Tensor` type, `@` for matrix
   multiplication, and reverse-mode differentiation for training models.
3. **Shapes in types.** Catch tensor dimension mismatches at check time.
4. **Data and models.** Data loading, batching, and loading pretrained
   weights.
5. **Tooling.** Language server, REPL, and a comment-preserving formatter.

## License

Not yet chosen.
