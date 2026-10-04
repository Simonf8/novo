import re
import os

def fix_file(filepath):
    with open(filepath, 'r', encoding='utf-8') as f:
        content = f.read()

    # Update enum
    content = content.replace("List(Vec<Value>)", "List(std::rc::Rc<std::cell::RefCell<Vec<Value>>>)")
    content = content.replace("Map(HashMap<String, Value>)", "Map(std::rc::Rc<std::cell::RefCell<HashMap<String, Value>>>)")
    content = content.replace("Map(std::collections::HashMap<String, Value>)", "Map(std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, Value>>>)")

    # Update constructions
    content = re.sub(r'Value::List\(([^R].*?)\)', r'Value::List(std::rc::Rc::new(std::cell::RefCell::new(\1)))', content)
    content = re.sub(r'Value::List\(\s*vec!\[\]\s*\)', r'Value::List(std::rc::Rc::new(std::cell::RefCell::new(vec![])))', content)
    
    content = re.sub(r'Value::Map\(([^R].*?)\)', r'Value::Map(std::rc::Rc::new(std::cell::RefCell::new(\1)))', content)

    # Note: the above regexes might match already converted ones if we're not careful, but [^R] prevents matching `Rc::new`.
    # Wait, `Value::List(l)` in match statements should NOT be wrapped in Rc::new.
    # So we should only wrap in Rc::new where we are CREATING a Value::List.
    # Where do we create? `Ok(Value::List(results))`, `self.stack.push(Value::List(list))`, etc.
    # Let's just do it manually. It's much safer!
    pass
