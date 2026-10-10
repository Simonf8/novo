<div align="center">

<img src="assets/logo.svg" alt="Novo" width="128" height="128" />

# Novo

**A statically typed language that reads like Python and runs like Rust.**

[![CI](https://github.com/Simonf8/novo/actions/workflows/ci.yml/badge.svg)](https://github.com/Simonf8/novo/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Rust 1.96+](https://img.shields.io/badge/rust-1.96%2B-orange.svg)
![Status: early development](https://img.shields.io/badge/status-early%20development-yellow.svg)

[Install](#install) · [Quick start](#quick-start) · [Tour](#a-short-tour) · [Machine learning](#machine-learning) · [Tooling](#tooling) · [How it works](#how-it-works)

</div>

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

## Why Novo

- **Easy to read, checked before it runs.** Full type inference means you rarely write a type, yet every program is checked ahead of time. Errors point at the exact code and suggest a fix.
- **Native speed.** Functions compile to machine code with [Cranelift](https://cranelift.dev). Numbers live in registers, and lists of numbers are stored unboxed.
- **Built for machine learning.** Tensors, autograd, optimizers and transformer layers are in the standard library, with GPU support through CUDA and Metal. Many tensor shape errors are caught at compile time.
- **Runs modern language models.** Load Llama-family checkpoints from Hugging Face and generate text directly.
- **Batteries included.** One binary gives you a REPL, formatter, test runner, language server and standalone executable builder.

## Performance

Measured with the programs in [`benches/`](benches):

| Benchmark | Rust `-O` | **Novo** | CPython 3.13 |
|:--|--:|--:|--:|
| `fib(32)` | 0.013 s | **0.026 s** | 0.322 s |
| Nested integer loops | 0.100 s | **0.123 s** | 1.585 s |
| Mandelbrot 600×600 | 0.090 s | **0.101 s** | 3.445 s |
| Numeric lists (sieve, prefix sums, dot product) | 0.054 s | **0.37 s** | 2.57 s |
| Map and list updates | | **0.26 s** | 0.45 s |

## Install

Prebuilt releases cover Linux (x86-64 and ARM), macOS on Apple silicon, and Windows. Both installers check the download against the release's SHA-256 sums.

```bash
# Linux and macOS
curl -fsSL https://raw.githubusercontent.com/Simonf8/novo/master/install.sh | sh
```

```powershell
# Windows
irm https://raw.githubusercontent.com/Simonf8/novo/master/install.ps1 | iex
```

### From source

Requires Rust 1.96 or newer.

```bash
git clone https://github.com/Simonf8/novo
cd novo
cargo build --release
```

Optional features:

| Feature | Adds |
|:--|:--|
| `python` | Import Python modules with `use python numpy as np` |
| `cuda` | Tensors on NVIDIA GPUs (compute capability 8.0 or newer with CUDA 13) |
| `metal` | Tensors on Apple GPUs |
| `accelerate` | Faster CPU tensors on macOS |
| `mkl` | Faster CPU tensors with Intel MKL |

```bash
cargo build --release --features cuda
```

## Quick start

```bash
novo new hello                # create a project
cd hello
novo run src/main.nv          # check and run it
novo test                     # run its tests
novo build src/main.nv -o app # make a standalone executable
```

Or start the interactive prompt with `novo`.

## A short tour

```novo
# Variables take the type of their first value.
names = ["Ada", "Grace"]
ages = {"Ada": 36, "Grace": 45}

# f-strings support Python format specs.
show f"{len(names)} people, average age {ages.values() average:.1f}"

for name in names:
    if ages[name] > 40:
        show name, "is over 40"
    otherwise:
        show name

# Functions are generic; closures capture by value.
function apply_twice(f, x) => f(f(x))
show apply_twice(|n| n * 2, 5)          # 20
show apply_twice(|s| s + "!", "hi")     # hi!!

# Queries over lists
evens = range(10) where it % 2 == 0
show evens select it * it               # [0, 4, 16, 36, 64]
show evens sum                          # 20
```

<details>
<summary><b>Errors, interfaces, generics and concurrency</b></summary>

```novo
# Errors carry a message or a value of your own type.
class NotFound:
    path: String

try:
    fail(NotFound("data.csv"))
catch NotFound as e:
    show "missing", e.path              # missing data.csv
catch e:
    show "caught:", e

# Interfaces and type parameters
interface Shape:
    function area() -> Float

class Square(Shape):
    side: Float
    function area() -> Float => self.side * self.side

function first[T](xs: List[T]) -> T => xs[0]
show first([Square(2.0)]).area()        # 4.0

# Loops and concurrency
count = 0
repeat 3 times:
    count += it                         # `it` is the loop index

task square(n) => n * n
show run [square(2), square(3)] in parallel     # [4, 9]
handle = together square(12)
show await handle                               # 144
```

</details>

The precise rules for numbers, scoping, classes, errors and display are in [docs/DESIGN.md](docs/DESIGN.md).

## Machine learning

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

Tensor types carry shapes where they can be known, so many dimension mismatches are caught before the program runs:

```python
function linear(x: Tensor[B, I], w: Tensor[I, O]) -> Tensor[B, O]:
    return x @ w

h = linear(nn.randn([32, 784]), nn.randn([784, 128]))   # Tensor[32, 128]
show h @ nn.randn([64, 10])
# error: cannot multiply shapes Tensor[32, 128] and Tensor[64, 10]: inner dimensions 128 and 64 differ
```

### Language models

`use llama` loads Llama-family checkpoints from Hugging Face: Llama 2 and 3, Qwen 2 and 2.5, SmolLM, Mistral, TinyLlama and others with the same layout. It reads `config.json`, single or sharded safetensors and the model's `tokenizer.json`, and generates with a KV cache and temperature, top-k and top-p sampling.

```python
use llama

model = llama.load("path/to/SmolLM2-135M")
show model.generate("The capital of France is", 20)
```

```bash
novo run examples/llm/generate.nv path/to/model "Once upon a time" 50 0.7 cuda bf16
```

Small Llama 3 and Qwen 2 models are tested against Hugging Face `transformers`: logits agree to within 1e-4 and greedy generation is identical. On a 4-core CPU, a model with SmolLM-135M's shape generates 42 tokens/s against PyTorch's 32, with identical output.

### On a GPU

On an NVIDIA L4, SmolLM2-360M generates 67.6 tokens/s in bf16 (30.7 on the CPU), and a 2048×2048 matrix multiply is about 15× faster than on the CPU. To try it without a GPU of your own, open [notebooks/gpu.ipynb](notebooks/gpu.ipynb) in Google Colab with an L4 or A100 runtime and run every cell.

### Examples

| Example | What it shows |
|:--|:--|
| [`linear_regression.nv`](examples/linear_regression.nv) | Gradient descent on a single feature |
| [`xor_mlp.nv`](examples/xor_mlp.nv) | A small neural network written as a Novo class |
| [`tiny_gpt.nv`](examples/tiny_gpt.nv) | A character-level GPT that trains from scratch in seconds |
| [`housing/`](examples/housing) | Reading a CSV file and training on shuffled minibatches |
| [`llm/`](examples/llm) | Text generation with pretrained models |
| [`gpu/`](examples/gpu) | Checking GPU results against the CPU |

## Tooling

```text
novo run <file.nv> [args...]     Check and run a program (--interp skips native code)
novo check <file.nv> [--json]    Type-check without running
novo test [path]                 Run test_*.nv and *_test.nv files
novo bench <file.nv> [-n N]      Time repeated runs of a program
novo new <name>                  Create a project
novo build <file.nv> [-o out]    Make a standalone executable
novo fmt [paths...] [--check]    Format files in place, or list unformatted ones
novo [repl]                      Start the interactive prompt
novo lsp                         Run the language server on stdio
```

- **Editors.** `novo lsp` gives errors as you type, hover types, go to definition, find references, rename, completion and formatting. [editors/vscode](editors/vscode) is a VS Code extension built on it.
- **Standalone programs.** `novo build` writes one executable that includes every imported file and runs on machines without Novo installed.
- **Formatter.** `novo fmt` normalizes indentation and spacing, keeps comments and intentional line breaks, and refuses to write a file whose tokens would change.
- **Standard library.** `math`, `json`, `csv`, `fs`, `env`, `time`, `random`, `tokenizer`, `nn`, `layers` and `llama`, plus `use "path/to/file.nv" as m` for your own modules.

## How it works

```text
                                                          ┌──► interpreter   (src/eval)
source ──► lexer/parser ──► type checker ──► typed HIR ───┤
           (src/syntax)     (src/check)                   └──► native code   (src/native)
```

| Part | Role |
|:--|:--|
| [`src/syntax`](src/syntax) | Indentation-aware lexer and recursive-descent parser that keep a source span on every node |
| [`src/check`](src/check) | Name resolution and Hindley–Milner style inference with let-polymorphism; `Any` is a gradual escape hatch for JSON and Python values |
| [`src/eval`](src/eval) | Reference interpreter over the typed HIR, with overflow-checked integers and insertion-ordered maps |
| [`src/native`](src/native) | Cranelift JIT that compiles each generic function once per concrete type, checks overflow, division by zero and recursion depth, and reports errors exactly as the interpreter does |
| [`src/diagnostic.rs`](src/diagnostic.rs) | Error rendering with source snippets, labels and help text, plus JSON output |

Set `NOVO_NATIVE_LOG=1` to see what compiles.

## Development

```bash
cargo test
```

Each program in `tests/programs` runs both natively and with `--interp`, and both outputs must match its `.out` file. After an intentional output change, regenerate them with `NOVO_BLESS=1 cargo test`.

`tools/fuzz.py` finds bugs the golden tests miss: `diff` checks that native code and the interpreter agree on random programs, `mutate` feeds corrupted programs to the checker and formatter, and `reduce` shrinks a failing case. CI runs the tests on Linux, macOS and Windows, and the fuzzer with a new seed on every run.

Release notes are in [CHANGELOG.md](CHANGELOG.md).

## Roadmap

1. **Faster native code:** closures and parallel blocks in compiled code, and inlining.
2. **Models:** more pretrained architectures, and loading them into `layers`.
3. **Tooling:** a package manager.

## License

Novo is released under the [MIT License](LICENSE).
