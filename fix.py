import re
import os

def process_file(path):
    with open(path, 'r', encoding='utf-8') as f:
        content = f.read()

    # Value::List( x ) -> Value::List(Box::new( x ))
    # But only if it's not in a match arm pattern like `Value::List(items) =>`
    # We can just match Value::List(...) when it's being constructed.
    # It's constructed usually like Value::List(vec) or Value::List(results)
    
    # We'll use a safer approach: try to compile and capture the compiler errors to apply patches automatically?
    # No, it's simpler to do simple regexes.
    
    # List construction
    content = re.sub(r'Value::List\((vec|results|list|res|result|mapped|new_list)\)', r'Value::List(Box::new(\1))', content)
    # Map construction
    content = re.sub(r'Value::Map\((map|module_map)\)', r'Value::Map(Box::new(\1))', content)
    
    # Function body construction
    content = re.sub(r'body: body\.clone\(\)', r'body: Box::new(body.clone())', content)
    content = re.sub(r'body: block,', r'body: Box::new(block),', content)
    
    # CompiledFunction chunk
    content = re.sub(r'chunk: function_compiler\.chunk,', r'chunk: Box::new(function_compiler.chunk),', content)
    
    # Pattern matching fixes
    content = re.sub(r'Value::List\(ref items\)', r'Value::List(items)', content)
    
    with open(path, 'w', encoding='utf-8') as f:
        f.write(content)

process_file('src/interpreter.rs')
process_file('src/vm/mod.rs')
process_file('src/compiler/mod.rs')
