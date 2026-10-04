import re

def fix(filepath):
    with open(filepath, 'r', encoding='utf-8') as f:
        code = f.read()

    # 1. Update Enum definition
    code = code.replace(
        "List(Vec<Value>)", 
        "List(std::rc::Rc<std::cell::RefCell<Vec<Value>>>)"
    )
    code = code.replace(
        "Map(std::collections::HashMap<String, Value>)", 
        "Map(std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, Value>>>)"
    )
    
    # 2. Update matches `Value::List(l) =>` -> `Value::List(l) => { let l = l.borrow(); ... }`
    # Wait, it's easier to just do `Value::List(ref l)` or `Value::List(l)`.
    # If we just replace Value::List(vec) with Value::List(rc_vec), we can access it with rc_vec.borrow().
    
    # Let's write the file back
    with open(filepath, 'w', encoding='utf-8') as f:
        f.write(code)

fix("src/interpreter.rs")
fix("src/vm/mod.rs")
fix("src/compiler/mod.rs")
