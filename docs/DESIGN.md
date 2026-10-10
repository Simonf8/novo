# Novo Language Design

This document fixes the semantics that the checker and every engine implement.
The golden programs in `tests/programs` are the executable form of it.

## Types

Novo is statically typed with full inference. Annotations are optional.

| Type          | Meaning                                    | Annotation              |
|---------------|--------------------------------------------|-------------------------|
| `Int`         | 64-bit signed integer                      | `Int`, `int`            |
| `Float`       | 64-bit IEEE float                          | `Float`, `float`        |
| `Bool`        | `true` / `false`                           | `Bool`, `bool`          |
| `String`      | immutable UTF-8 text                       | `String`, `str`         |
| `None`        | the unit value `none`                      | `None`                  |
| `List[T]`     | growable list, shared by reference         | `List[T]`               |
| `Map[K, V]`   | insertion-ordered map, shared by reference | `Map[K, V]`             |
| tuples        | fixed-size immutable groups of values      | `(Int, String)`         |
| `Task[T]`     | a computation running in the background    | `Task[T]`               |
| `Tensor`      | an n-dimensional array of 32-bit floats    | `Tensor`                |
| `Optimizer`   | updates parameters from a loss             | `Optimizer`             |
| classes       | user-defined records with methods          | the class name          |
| functions     | `(A, B) -> R`                              | inferred only           |
| `Any`         | a dynamically typed value                  | `Any`                   |

A variable's type is fixed by its first assignment. Assigning a `Float` to an
`Int` variable is an error; write `0.0` when a counter will hold floats.

Named functions are generic: `function apply(f, x) => f(x)` works for any `f`
and `x` that fit. Parameters and return types are inferred from use.

`Any` is the escape hatch for data whose shape is only known at run time: JSON
documents and Python objects. Any operation on an `Any` is checked when it
runs, and an `Any` can be passed wherever a concrete type is expected, with a
run-time check at that point. Code that never touches `Any` is fully checked
before it runs.

## Numbers

- `Int` arithmetic is checked: overflow is a run-time error, never a wrap.
- `/` always produces a `Float`. `//` is floor division and keeps `Int`
  when both sides are `Int`. `%` takes the sign of the divisor.
- Mixing `Int` and `Float` in an operator produces a `Float`.
- Dividing by zero, with `/`, `//` or `%`, is a run-time error.
- `Int ** Int` is an `Int`; a negative exponent is a run-time error.
- `round` rounds halves to even, like Python: `round(2.5)` is `2`.

## Conditions and operators

Conditions (`if`, `while`, `where`) must be `Bool`; there is no truthiness.
`and`, `or` and `not` take `Bool` and short-circuit. `==` compares lists, maps
and strings by value and class instances by identity. `<` and friends work on
numbers and strings. `x in c` tests list membership, map keys or substrings.
Comparisons do not chain.

`a if cond else b` evaluates to `a` when `cond` holds and to `b` otherwise
(`otherwise` may be written for `else`). Both branches must have the same
type, except that an `Int` and a `Float` give a `Float`.

## Variables and scope

A function body is one scope: blocks do not introduce new variables. Code in a
function can read and assign the module's top-level variables. Closures
(`|x| x + n`) and nested functions capture the variables they use by value
when they are created.

A variable must be assigned on every path before it is read; `novo check`
reports a read that might come first. Paths through `if` branches, loops that
may run zero times, and a `try` whose body may fail before the assignment all
count. A function can still run before top-level code has assigned a module
variable it reads; that read is a run-time error.

`x op= value` reads `x` before evaluating `value`.

## Control flow

`if` / `otherwise if` / `otherwise`, `while`, `for x in xs`, and
`repeat n times`, where the loop index is available as `it`. `for` iterates
over lists, the characters of a string, and the keys of a map. `break` and
`continue` apply to the innermost loop.

## Tuples

`(1, "a")` is a tuple; `return a, b` returns one. Tuples unpack in
assignments (`a, b = b, a`, nested patterns too) and in loops
(`for name, age in ages.items()`). Index a tuple with a number written in the
code (`pair[0]`) so its element type is known. Tuples compare element by
element.

## Collections

Indexing accepts negative indices counted from the end. Indexing out of range,
or reading a missing map key, is a run-time error. Slices `xs[a:b]` work on
lists and strings and clamp to the bounds.

Query operators work on lists. A body is either a closure (`xs where |x| x > 1`)
or an expression that uses `it` (`xs where it > 1`):

- `where`: keep the items whose body is `true`
- `select`: map each item through the body
- `sum`, `min`, `max`, `average`: aggregates, optionally of a body;
  `average` is always a `Float`, and `min`/`max` of an empty list is an error

Built-in functions: `len`, `range`, `str`, `int`, `float`, `bool`, `abs`,
`min`, `max`, `round`, `sum`, `any`, `all`, `sorted` (with an optional key
function), `reversed`, `enumerate`, `zip`, `chr`, `ord`, `from_bytes`
(UTF-8, replacing invalid sequences), `type_of`, `input`, `assert`,
`assert_eq` and `fail`. Strings have `bytes()` and the character tests
`is_alpha`, `is_digit`, `is_space`, `is_alnum`, `is_upper` and `is_lower`.

## Classes

```novo
class Counter:
    value: Int = 0
    step = 1

    function increment():
        self.value += self.step
```

Fields are declared with a type, a default, or both. `Counter()` builds an
instance from the defaults; fields can be set positionally, in declaration
order, or by name: `Counter(value=5)`. Methods see the instance as `self`.
Instances are shared by reference.

## Errors

Run-time errors (division by zero, a bad index, a failed `assert`, a call to
`fail(message)`) unwind to the nearest `try`. In `catch e`, `e` is the error
message as a `String`. Uncaught errors print a diagnostic and exit with
status 1.

## Modules

`use "path/file.nv" as m` loads another file relative to the current one,
runs its top-level code once, and exposes its top-level names as `m.name`.
Standard modules (`math`, `time`, `json`, `csv`, `fs`, `env`, `random`, `nn`) are brought in with
`use math`. `use python numpy as np` imports a Python module as an `Any`;
it requires building with `--features python`.

## Concurrency

- `task` is another spelling of `function`, for code meant to run in the background.
- `together expr` starts `expr` on another thread and returns a `Task[T]`;
  `await t` waits for it and returns the `T`.
- `run [a, b, c] in parallel` and `[a, b, c] together` evaluate every element
  concurrently and return the list of results.
- `(xs select body) together` maps in parallel.

## Tensors

`use nn` brings in tensors, built on candle:

- Creation: `nn.tensor(nested_lists)`, `nn.zeros(shape)`, `nn.ones(shape)`,
  `nn.randn(shape)`, `nn.rand(shape)`, `nn.arange(n)`, `nn.eye(n)`,
  `nn.tril(n)`, `nn.stack(tensors, dim)` and `nn.cat(tensors, dim)`.
  `nn.seed(n)` makes random tensors reproducible.
- Operators: `+ - * /` broadcast between tensors and with numbers, `**` takes
  a number exponent, `@` is matrix multiplication (vectors are treated as rows
  or columns as needed), and unary `-` negates. Comparisons are not operators;
  use `a.eq(b)`, which returns a tensor of ones and zeros.
- Indexing: `t[i]` selects along the first dimension and `t[a:b]` narrows it.
  `len(t)`, `for row in t` and queries (`t select ...`) work on rows.
- Methods: `shape`, `rank`, `size`, `item`, `to_list`, `sum`/`mean` (over
  everything or one dimension, optionally keeping it), `max`, `min`, `argmax`,
  `argmin`, `softmax`, `log_softmax`, `relu`, `sigmoid`, `tanh`, `gelu`,
  `silu`, `exp`, `log`, `sqrt`, `abs`, `square`, `pow`, `clamp`, `reshape`
  (one dimension may be `-1`), `flatten`, `t`, `transpose`, `squeeze`,
  `unsqueeze`, `narrow`, `chunk`, `matmul`, `eq`, `detach` and `copy`.
- Model building blocks: `table.embed(ids)` looks up rows by index,
  `x.masked_fill(mask, value)` replaces entries where the mask is non-zero,
  `x.layer_norm(weight, bias)` normalizes over the last dimension, and
  `probs.sample()` draws an index from a vector of weights.
- Weights: `nn.save(map_of_tensors, path)` and `nn.load(path)` read and write
  the safetensors format.
- Training: `nn.param(t)` marks a tensor as trainable. `loss.backward()`
  computes gradients, readable with `p.grad()`. `nn.mse` and
  `nn.cross_entropy` are the losses; cross-entropy takes class indices as a
  tensor of whole numbers.
- Optimizers: `nn.sgd(params, lr, momentum)`, `nn.adam(params, lr)` and
  `nn.adamw(params, lr, weight_decay)` (default 0.01) match PyTorch's. Their
  `step(loss)` runs backward and updates the parameters. `opt.clip(max_norm)`
  scales gradients down to a global norm, `opt.grad_norm()` reports the norm
  of the latest step before clipping, and `opt.lr()` / `opt.set_lr(lr)` read
  and change the learning rate. `nn.cosine_lr(step, warmup, total, max_lr,
  min_lr)` gives the usual warmup-then-cosine schedule.
- Checkpoints: `nn.save_params(params, path)` and `nn.load_params(params,
  path)` write and restore a list of parameters in place; `opt.save(path)`
  and `opt.load(path)` do the same for the optimizer's state, so training can
  resume exactly where it stopped.

- Devices and precision: tensors live on the CPU unless `nn.device("cuda")`
  or `nn.device("metal")` selects a GPU (Novo must be built with the `cuda`
  or `metal` feature). `t.to("cpu")` moves a tensor, `t.to_dtype("bf16")`
  changes its precision (`f32`, `f16`, `bf16`, `f64`), and `nn.load(path,
  "bf16")` loads weights in a given precision.

### Shapes

Tensor types carry shapes when they can be known: `nn.zeros([2, 3])` is a
`Tensor[2, 3]`, `nn.tensor` of a nested list literal has the literal's shape,
and arithmetic, `@`, indexing, iteration and most methods (`t`, `transpose`,
`reshape`, `sum`, `mean`, `max`, `argmax`, `squeeze`, `unsqueeze`, `narrow`,
`flatten`, `embed`, element-wise functions) compute the shape of their result.
Mismatches that would certainly fail at run time are reported by `novo check`:

```
x = nn.randn([8, 3])
w = nn.randn([4, 2])
show x @ w      # error: cannot multiply shapes Tensor[8, 3] and Tensor[4, 2]
```

Annotations can state shapes. A number is a fixed size, `_` is any size, and a
name stands for the same size everywhere it appears in one function, so
functions can be generic over sizes:

```
function linear(x: Tensor[B, I], w: Tensor[I, O], b: Tensor[O]) -> Tensor[B, O]:
    return x @ w + b
```

Plain `Tensor` means a tensor of unknown shape and is compatible with every
shape. Because a variable has one type for its whole life, only variables
assigned in one place keep shapes; values stored in lists, maps, fields,
variables assigned in several places, or returned from several `return`
statements are just `Tensor`. Shape errors that the checker cannot see are
run-time errors that name both shapes.

`use random` gives `random.int(low, high)` (high excluded), `random.float()`,
`random.choice(list)`, `random.shuffle(list)` and `random.seed(n)`. It shares
one generator with `nn`, so a single seed makes a training run reproducible.

### Layers

`use layers` is a standard module written in Novo with the usual building
blocks. Each layer has `forward(x)` and `params()`:

- `layers.linear(inputs, outputs)`: `x @ weight + bias`.
- `layers.embedding(count, width)`: looks up rows of a table by index.
- `layers.layer_norm(width)`: normalizes the last dimension.
- `layers.mlp(width, hidden)`: two linear layers with a GELU between them.
- `layers.attention(width, heads)`: causal multi-head self-attention over
  `[batch, steps, width]` inputs; `layers.causal_mask(steps)` is its mask.
- `layers.transformer_block(width, heads)`: a pre-norm block of attention
  and an MLP with residual connections.

The classes (`layers.Linear`, `layers.TransformerBlock`, ...) can be named in
type annotations to build models out of them.

## Data

`use csv` reads and writes comma-separated values, with quoted fields as in
RFC 4180. `csv.read(path)` and `csv.parse(text)` return a `List[List[String]]`
of rows; `csv.write(path, rows)` and `csv.format(rows)` go the other way.

`fs.read_lines_from(path, offset, count)` returns up to `count` lines starting
at byte `offset`, and the offset to continue from, so a large text or JSONL
file can be processed a piece at a time without loading it whole.

`use tokenizer` reads tokenizers in the Hugging Face `tokenizer.json` format,
which is how current models (Llama, Qwen, Mistral, SmolLM and others) publish
theirs: `tokenizer.load(path)` takes the file or the model's directory.
`tokenizer.train(texts, vocab_size, special_tokens)` trains a byte-level BPE
tokenizer on your own text. A `Tokenizer` has `encode(text)` (pass `false` to
leave out the model's special tokens such as a start-of-text marker),
`decode(ids)` (pass `false` to keep special tokens), `vocab_size()`,
`id(token)` (`-1` if absent), `token(id)` and `save(path)`.

Tokenized datasets are stored as binary files of 32-bit ids:
`nn.save_tokens(ids, path)` writes one and `nn.load_tokens(path)` reads it as
a 1-D tensor. `nn.sample_windows(tokens, batch, context)` picks `batch`
random windows of `context` tokens and returns the inputs and, shifted by one
token, the targets, ready for next-token prediction.

For training on minibatches, `t.take(indices)` gathers rows by index and
`t.batches(size)` splits a tensor into consecutive pieces along its first
dimension, the last one possibly shorter. Shuffling the same index list into
the inputs and targets keeps them aligned:

```
order = range(n)
random.shuffle(order)
for xb, yb in zip(x.take(order).batches(32), y.take(order).batches(32)):
    opt.step(nn.mse(model(xb), yb))
```

## Display

`show a, b` prints its values separated by spaces. Floats always print with a
decimal point (`10.0`), `none` prints as `none`, lists as `[1, 2]`, maps as
`{a: 1}` in insertion order, and instances as `Counter(value: 1, step: 1)`.
Strings inside collections print without quotes.
