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

## Variables and scope

A function body is one scope: blocks do not introduce new variables. Code in a
function can read and assign the module's top-level variables. Closures
(`|x| x + n`) and nested functions capture the variables they use by value
when they are created.

## Control flow

`if` / `otherwise if` / `otherwise`, `while`, `for x in xs`, and
`repeat n times`, where the loop index is available as `it`. `for` iterates
over lists, the characters of a string, and the keys of a map. `break` and
`continue` apply to the innermost loop.

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
Standard modules (`math`, `time`, `json`, `fs`, `env`) are brought in with
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
  computes gradients, readable with `p.grad()`. `nn.sgd(params, lr)` and
  `nn.adam(params, lr)` create optimizers whose `step(loss)` runs backward and
  updates the parameters. `nn.mse` and `nn.cross_entropy` are the losses;
  cross-entropy takes class indices as a tensor of whole numbers.

Shape errors are run-time errors that name both shapes.

`use random` gives `random.int(low, high)` (high excluded), `random.float()`,
`random.choice(list)`, `random.shuffle(list)` and `random.seed(n)`. It shares
one generator with `nn`, so a single seed makes a training run reproducible.

## Display

`show a, b` prints its values separated by spaces. Floats always print with a
decimal point (`10.0`), `none` prints as `none`, lists as `[1, 2]`, maps as
`{a: 1}` in insertion order, and instances as `Counter(value: 1, step: 1)`.
Strings inside collections print without quotes.
