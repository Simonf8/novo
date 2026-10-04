import os
import subprocess
import glob
import sys

def main():
    test_files = glob.glob("tests/*.nv")
    test_files.extend(glob.glob("try_test.nv"))
    test_files.sort()
    
    success = True
    
    for f in test_files:
        print(f"Running {f}...")
        
        # Run with AST
        ast_proc = subprocess.run(
            ["C:\\Users\\kefet\\.cargo\\bin\\cargo.exe", "run", "--quiet", "--", "run", "--engine=ast", f],
            capture_output=True, text=True,
            env={**os.environ, "PATH": os.environ.get("PATH", "") + ";C:\\Users\\kefet\\AppData\\Roaming\\uv\\python\\cpython-3.11.14-windows-x86_64-none"}
        )
        
        # Run with VM
        vm_proc = subprocess.run(
            ["C:\\Users\\kefet\\.cargo\\bin\\cargo.exe", "run", "--quiet", "--", "run", "--engine=vm", f],
            capture_output=True, text=True,
            env={**os.environ, "PATH": os.environ.get("PATH", "") + ";C:\\Users\\kefet\\AppData\\Roaming\\uv\\python\\cpython-3.11.14-windows-x86_64-none"}
        )
        
        ast_out = ast_proc.stdout.strip()
        vm_out = vm_proc.stdout.strip()
        
        import re
        ast_out_clean = re.sub(r" at line \d+, col \d+, len \d+", "", ast_out)
        vm_out_clean = re.sub(r" at line \d+, col \d+, len \d+", "", vm_out)

        if ast_out_clean != vm_out_clean:
            print(f"FAIL: {f}")
            print(f"AST Output:\n{ast_out}")
            if ast_proc.stderr.strip(): print(f"AST Stderr:\n{ast_proc.stderr.strip()}")
            print(f"VM Output:\n{vm_out}")
            if vm_proc.stderr.strip(): print(f"VM Stderr:\n{vm_proc.stderr.strip()}")
            success = False
        else:
            print(f"PASS: {f}")
            
    if success:
        print("All tests passed!")
        sys.exit(0)
    else:
        print("Some tests failed.")
        sys.exit(1)

if __name__ == "__main__":
    main()
