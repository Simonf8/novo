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

Every function compiles to machine code with [Cranelift](https://cranelift.dev)
the first time it is called. Numbers stay in registers; strings, lists, maps
and objects are handled by a runtime shared with the reference interpreter,
so both behave identically. Lists of Ints, Floats or Bools store their
elements unboxed, and loops that only do arithmetic on them read and write
the elements in place.

| Benchmark (`benches/`) | Rust `-O` | Novo | CPython 3.13 |
|------------------------|----------:|-----:|-------------:|
| `fib(32)`              | 0.013 s   | 0.026 s | 0.322 s |
| nested integer loops   | 0.100 s   | 0.123 s | 1.585 s |
| Mandelbrot 600×600     | 0.090 s   | 0.101 s | 3.445 s |
| numeric lists (sieve, prefix sums, dot product) | 0.054 s | 0.37 s | 2.57 s |
| map and list updates   |           | 0.26 s  | 0.45 s  |

> **Status:** early development. See the [roadmap](#roadmap).

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
cargo build --release --features accelerate  # faster CPU tensors on macOS
cargo build --release --features mkl      # faster CPU tensors with Intel MKL
```

## Usage

```text
novo run <file.nv> [args...]     Check and run a program
         [--interp]              (without compiling anything to native code)
novo check <file.nv> [--json]    Type-check without running
novo test [path]                 Run test_*.nv and *_test.nv files
novo bench <file.nv> [-n N]      Time repeated runs of a program
novo new <name>                  Create a project
novo fmt [paths...] [--check]    Format files in place (or list unformatted ones)
novo [repl]                      Start the interactive prompt
novo lsp                         Run the language server on stdio
```

`novo check --json` prints one JSON object per diagnostic, for editors.
`novo lsp` is a language server: errors as you type, hover types, go to
definition and formatting. [editors/vscode](editors/vscode) is a VS Code extension that uses
it and adds syntax highlighting.

`novo fmt` normalizes indentation and spacing but keeps comments, blank lines
(up to two in a row) and line breaks inside brackets. It refuses to write a
file if the result would not read back as the same tokens.

At the interactive prompt each input runs against the state the earlier ones
left, and a bare expression shows its value. Defining a function again
replaces it. An input that fails to check is forgotten entirely.

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

Tensor types carry shapes where they can be known, so many dimension
mismatches are caught before the program runs:

```python
function linear(x: Tensor[B, I], w: Tensor[I, O]) -> Tensor[B, O]:
    return x @ w

h = linear(nn.randn([32, 784]), nn.randn([784, 128]))   # Tensor[32, 128]
show h @ nn.randn([64, 10])
# error: cannot multiply shapes Tensor[32, 128] and Tensor[64, 10]: inner dimensions 128 and 64 differ
```

### Running modern language models

`use llama` loads Llama-family checkpoints from Hugging Face as they are
published: Llama 2 and 3, Qwen 2 and 2.5, SmolLM, Mistral, TinyLlama and
others with the same layout. It reads `config.json`, single or sharded
safetensors, and the model's own `tokenizer.json`, and generates with a KV
cache and temperature, top-k and top-p sampling:

```python
use llama

model = llama.load("path/to/SmolLM2-135M")
show model.generate("The capital of France is", 20)
```

```bash
novo run examples/llm/generate.nv path/to/model "Once upon a time" 50 0.7
```

Tiny random Llama 3 and Qwen 2 models are tested against Hugging Face
`transformers`: logits agree to within 1e-4 and greedy generation is
identical. On a 4-core CPU, a model with SmolLM-135M's shape generates
**42 tokens/s against PyTorch's 32** with identical output (the speed comes
from a multi-threaded matrix-vector kernel for single-token steps, fused
projections, and attention that never copies the key/value cache per head).

[examples/xor_mlp.nv](examples/xor_mlp.nv) trains a small neural network
written as a Novo class, and [examples/tiny_gpt.nv](examples/tiny_gpt.nv) is a
character-level GPT built from the `layers` standard module (embeddings,
causal self-attention, layer norm, MLP blocks) that trains from scratch and
generates text in a few seconds.
[examples/housing](examples/housing) reads a CSV file and fits a model on
shuffled minibatches.

Modules: `use "path/to/file.nv" as m` loads another file; `use math`,
`use json`, `use csv`, `use tokenizer`, `use time`, `use fs`, `use env`, `use random`, `use nn` and `use layers` bring in
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
  with an `Int` compiles to an `Int`-only function. `Int`, `Float` and `Bool`
  live in registers; other values live in slots of the native frame and are
  operated on by runtime helpers that reuse the interpreter's code. Top-level
  variables that no function touches are kept in registers too. Compiled code
  checks overflow, division by zero and recursion depth, and reports errors
  exactly as the interpreter does. Set `NOVO_NATIVE_LOG=1` to see what
  compiles.
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

`tools/fuzz.py` looks for bugs the golden tests miss. `diff` generates random
valid programs and checks that native code and the interpreter agree,
`mutate` feeds corrupted programs to the checker and formatter to make sure
they never crash, and `reduce` shrinks a failing program. CI runs both with a
new seed each time.

## Roadmap

1. **Faster native code.** Closures and parallel blocks in compiled code,
   unboxed lists of numbers, inlining, and ahead-of-time executables.
2. **Models.** More pretrained architectures, and loading them into `layers`.
3. **Tooling.** Completion and a package manager.

## License

Not yet chosen.
