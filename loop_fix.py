import json
import subprocess
import os

def run_check():
    cargo_path = os.path.expanduser('~/.cargo/bin/cargo')
    cmd = f'"{cargo_path}" check --message-format=json'
    result = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    return result.stdout

def apply_fixes():
    for _ in range(10): # max 10 iterations
        stdout = run_check()
        replacements = []
        for line in stdout.splitlines():
            try:
                msg = json.loads(line)
            except:
                continue
            if msg.get('reason') == 'compiler-message':
                err = msg.get('message', {})
                for child in err.get('children', []):
                    if child.get('message', '').startswith("call `Into::into`"):
                        for span in child.get('spans', []):
                            if 'suggested_replacement' in span:
                                replacements.append({
                                    'file': span['file_name'],
                                    'line': span['line_start'] - 1,
                                    'col': span['column_start'] - 1,
                                    'text': span['suggested_replacement']
                                })
                                
        if not replacements:
            print("No more Into::into replacements found.")
            break
            
        print(f"Applying {len(replacements)} replacements...")
        
        # apply only 1 fix per line to avoid offset issues
        files = {}
        for r in replacements:
            f = r['file']
            if f not in files:
                files[f] = {}
            l = r['line']
            if l not in files[f]:
                files[f][l] = r
                
        for filename, line_reps in files.items():
            with open(filename, 'r', encoding='utf-8') as f:
                content = f.readlines()
                
            for l, r in line_reps.items():
                text = content[l]
                col = r['col']
                content[l] = text[:col] + r['text'] + text[col:]
                
            with open(filename, 'w', encoding='utf-8') as f:
                f.writelines(content)

if __name__ == '__main__':
    apply_fixes()
