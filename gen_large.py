import sys

def gen_novo(filename, lines):
    with open(filename, 'w') as f:
        f.write("use python time\n")
        f.write("start = time.time()\n")
        f.write("a0 = 1\n")
        for i in range(1, lines):
            f.write(f"a{i} = a{i-1} + 1\n")
        f.write("end = time.time()\n")
        f.write("show end - start\n")

def gen_py(filename, lines):
    with open(filename, 'w') as f:
        f.write("import time\n")
        f.write("start = time.time()\n")
        f.write("a0 = 1\n")
        for i in range(1, lines):
            f.write(f"a{i} = a{i-1} + 1\n")
        f.write("end = time.time()\n")
        f.write("print(end - start)\n")

gen_novo("bench_1000.nv", 10000)
gen_py("bench_1000.py", 10000)
