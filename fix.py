import json

def fix_errors():
    with open('errors.json', 'r') as f:
        lines = f.readlines()
    
    fixes = {}
    
    for line in lines:
        try:
            msg = json.loads(line)
        except:
            continue
            
        if msg.get('reason') == 'compiler-message':
            err = msg.get('message', {})
            if err.get('code') and err['code'].get('code') == 'E0308': # mismatched types
                spans = err.get('spans', [])
                for span in spans:
                    if span.get('is_primary'):
                        filename = span['file_name']
                        start_line = span['line_start']
                        end_line = span['line_end']
                        
                        # Only handle single-line for simplicity
                        if start_line == end_line:
                            if filename not in fixes:
                                fixes[filename] = []
                            # We want to replace ExpressionNode::... with ExpressionNode::....into()
                            # or just append .into() to the expression.
                            # The span text might be `ExpressionNode::Integer(i)`
                            fixes[filename].append({
                                'line': start_line,
                                'start': span['column_start'] - 1,
                                'end': span['column_end'] - 1,
                                'text': span['text'][0]['text'] if span['text'] else ""
                            })
                            
    for filename, file_fixes in fixes.items():
        with open(filename, 'r') as f:
            file_lines = f.readlines()
        
        # apply fixes from end to start to not mess up columns
        # group by line
        line_fixes = {}
        for fix in file_fixes:
            l = fix['line'] - 1
            if l not in line_fixes:
                line_fixes[l] = []
            line_fixes[l].append(fix)
            
        for l, lfixes in line_fixes.items():
            lfixes.sort(key=lambda x: x['start'], reverse=True)
            text = file_lines[l]
            for fix in lfixes:
                # insert .into() after the span
                end = fix['end']
                text = text[:end] + ".into()" + text[end:]
            file_lines[l] = text
            
        with open(filename, 'w') as f:
            f.writelines(file_lines)

fix_errors()
