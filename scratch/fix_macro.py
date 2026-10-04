import sys, re

content = open('src/vm/mod.rs').read()
content = re.sub(
    r'macro_rules! throw \{\s*\(\$frame:expr, \$err:expr\) => \{\{',
    r'macro_rules! throw {\n            ($frame:expr, $label:lifetime, $err:expr) => {{\n                self.frames.push($frame);',
    content
)
content = re.sub(r'self.frames.push\(\$frame\);\n\s+if let Some', 'if let Some', content)
content = re.sub(r"continue 'vm_loop;", 'continue $label;', content)
content = re.sub(r'throw!\(frame,\s*', r"throw!(frame, 'vm_loop, ", content)

open('src/vm/mod.rs', 'w').write(content)
