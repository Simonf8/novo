# Changelog

## 0.2.0

A rewrite of the language around a static type checker and a native compiler.

### Language
- Static types with full inference; annotations are optional, and `Any` is
  the explicit escape hatch for dynamic data.
- Variables are checked to be assigned before they are read.
- Tuples, conditional expressions, and the common Python built-ins.
- Enums with `match`, checked to handle every variant.
- Interfaces that classes declare and are checked against, and type
  parameters for generic functions (`function first[T](xs: List[T]) -> T`).
- Typed errors: `fail` takes class or enum instances, and
  `catch ParseError as e` handles only that type.
- Runtime errors show the chain of calls they passed through.

### Performance
- All code compiles to native code with Cranelift; `--interp` runs the
  reference interpreter instead.
- Lists of numbers are stored unboxed and read in place by compiled loops.

### Machine learning
- Tensors with autograd, shape checking in types (`Tensor[32, 128]`), GPU
  and reduced-precision support through candle.
- Optimizers (SGD, Adam, AdamW) with momentum, gradient clipping, learning
  rate schedules and checkpoints.
- The `layers` module of transformer building blocks, safetensors loading,
  tokenizers, and token datasets.
- The `llama` module runs Llama-family checkpoints from Hugging Face (Llama,
  SmolLM, Qwen and similar) with a KV cache.
- A `csv` module and minibatch helpers.

### Tools
- `novo repl`, `novo fmt` (splitting long lines), `novo test`, `novo bench`.
- `novo build` writes standalone executables.
- `novo lsp`: diagnostics, hover, go to definition, references, rename and
  completion, with a VS Code extension.
- A fuzzer and differential tester that run in CI on every push.
- Prebuilt releases and install scripts.

## 0.1.0

The original tree-walking interpreter.
