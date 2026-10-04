# Novo Language Specification

Novo is a high-level, dynamically typed language that emphasizes readability, minimal boilerplate, and productivity. Its philosophy is: "Python makes you describe how. Novo lets you describe what." It features native list processing, deep Python interoperability, and now a complete suite of modern language constructs.

## 1. Syntax & Data Types
Novo supports standard data types:
- **Integers & Floats**: `10`, `3.14`
- **Booleans**: `true`, `false`
- **Strings**: `"Hello, World"`
- **Lists**: `[1, 2, 3]`
- **Maps (Objects/Records)**: `{ name: "Novo", version: 1.0 }`

### String Interpolation
Novo provides `f-strings` for easy string formatting:
```novo
name = "World"
greeting = f"Hello, {name}!"
show greeting
```

## 2. Variables & Operators
Variables are defined via assignment. Type annotations are optional and informational.
```novo
x = 10
y: int = 20
```

Supported operators:
- Arithmetic: `+`, `-`, `*`, `/`, `%`
- Comparison: `==`, `!=`, `<`, `>`, `<=`, `>=`
- Logical: `and`, `or`, `not`
- String Operations: `starts_with`, `ends_with`

## 3. Control Flow
### If Statements
Novo offers clean conditional logic using `if` and `otherwise`:
```novo
if count > 10:
    show "Large"
otherwise if count > 5:
    show "Medium"
otherwise:
    show "Small"
```

### Loops (Repeat & While)
**Repeat**: Run a block of code a specific number of times. The index is available as `it`.
```novo
repeat 5 times:
    show f"Iteration {it}"
```

**While**: Loop based on a condition. Supports `break` and `continue`.
```novo
count = 0
while count < 10:
    if count == 5:
        count = count + 1
        continue
    show count
    count = count + 1
```

## 4. Functions & Closures
Functions are declared using the `function` keyword.
```novo
function greet(name, greeting: str):
    return f"{greeting}, {name}!"
```

### Closures (Anonymous Functions)
Closures can be defined inline using pipes `|...|` and passed as expressions.
```novo
multiply = |x, y| x * y
show multiply(5, 4)
```

## 5. Classes & Custom Types
Classes provide a way to encapsulate behavior via custom types. They currently act as constructors grouping methods.
```novo
class Person:
    function greet():
        show "Hello!"
```

## 6. Error Handling
Novo provides robust error handling to gracefully handle failures without crashing the application.
```novo
try:
    result = 10 / 0
catch e:
    show f"An error occurred: {e}"
```

## 7. Module System
Code can be organized across multiple files and imported effortlessly.

### Importing Novo Modules
```novo
use "utils.nv" as utils
show utils.helper_function()
```

### Importing Python Modules
Due to Novo's native integration with Python, importing any standard Python library or package is simple.
```novo
use python math
show math.sqrt(16)

use python requests as req
response = req.get("https://api.github.com")
```

## 8. Collection Processing (Query Syntax)
Novo introduces SQL-like query operations for functional data transformation on lists and iterables.
- `where`: Filters a collection.
- `select`: Maps a collection.
- Aggregations: `sum`, `average`, `min`, `max`.

```novo
data = [1, 2, 3, 4, 5, 6]
evens = data where |x| x % 2 == 0
doubled = evens select |x| x * 2
total = doubled sum
show total
```

## 9. Parallel Execution
Tasks can be executed concurrently using the `together` keyword, spinning up threads automatically for collection items.
```novo
results = together [task1(), task2()]
```
