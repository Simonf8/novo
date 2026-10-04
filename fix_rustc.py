import json
import os

def apply_rustc_suggestions():
    with open('errors.json', 'r') as f:
        lines = f.readlines()
        
    replacements = []
    
    for line in lines:
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

    # Group by file
    files = {}
    for r in replacements:
        f = r['file']
        if f not in files:
            files[f] = []
        files[f].append(r)
        
    for filename, reps in files.items():
        with open(filename, 'r') as f:
            content = f.readlines()
            
        # Group by line
        line_reps = {}
        for r in reps:
            l = r['line']
            if l not in line_reps:
                line_reps[l] = []
            line_reps[l].append(r)
            
        for l, lreps in line_reps.items():
            # sort descending by column to not mess up indices
            lreps.sort(key=lambda x: x['col'], reverse=True)
            text = content[l]
            for r in lreps:
                col = r['col']
                text = text[:col] + r['text'] + text[col:]
            content[l] = text
            
        with open(filename, 'w') as f:
            f.writelines(content)
            
apply_rustc_suggestions()
