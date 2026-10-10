#!/usr/bin/env python3
"""Fuzzing for the Novo toolchain.

    tools/fuzz.py mutate [--count N] [--seed S]   corrupt test programs; check and fmt must not crash
    tools/fuzz.py diff [--count N] [--seed S]     random valid programs; native and --interp must agree
    tools/fuzz.py reduce FILE                     shrink a program that makes the engines disagree

Failing inputs are saved under --out (default target/fuzz). Exits 1 if any were found.
"""

import argparse
import glob
import os
import random
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TIMEOUT = 10


def run(novo, args, timeout=TIMEOUT):
    try:
        p = subprocess.run([novo] + args, capture_output=True, text=True, errors="replace", timeout=timeout)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return "timeout", "", ""


def crashed(result):
    code, _, err = result
    return code not in (0, 1, "timeout") or "panicked" in err


class IntPrograms:
    """Integer arithmetic, lists, calls between functions, try/catch."""

    def __init__(self, rng):
        self.r = rng
        self.funcs = []  # (name, nparams)

    def int_expr(self, vars, depth=0):
        r = self.r
        choices = ["lit", "var", "bin", "bin", "call", "cond", "len", "neg", "index"]
        if depth > 3:
            choices = ["lit", "var"]
        c = r.choice(choices)
        if c == "lit" or (c == "var" and not vars):
            return str(r.choice([0, 1, 2, 3, 7, 10, -1, -5, 100, 9223372036854775807, -9223372036854775807]))
        if c == "var":
            return r.choice(vars)
        if c == "bin":
            op = r.choice(["+", "-", "*", "//", "%", "+", "-"])
            return f"({self.int_expr(vars, depth+1)} {op} {self.int_expr(vars, depth+1)})"
        if c == "call" and self.funcs:
            name, n = r.choice(self.funcs)
            return f"{name}({', '.join(self.int_expr(vars, depth+1) for _ in range(n))})"
        if c == "cond":
            return f"({self.int_expr(vars, depth+1)} if {self.bool_expr(vars, depth+1)} else {self.int_expr(vars, depth+1)})"
        if c == "len":
            return "len(xs)"
        if c == "neg":
            return f"-{self.int_expr(vars, depth+1)}"
        if c == "index":
            return f"xs[{self.int_expr(vars, depth+1)}]"
        return "1"

    def bool_expr(self, vars, depth=0):
        r = self.r
        if depth < 3 and r.random() < 0.3:
            return f"({self.bool_expr(vars, depth+1)} {r.choice(['and', 'or'])} {self.bool_expr(vars, depth+1)})"
        return f"{self.int_expr(vars, depth+1)} {r.choice(['<', '<=', '>', '>=', '==', '!='])} {self.int_expr(vars, depth+1)}"

    def block(self, vars, indent, depth, in_func):
        r = self.r
        lines = []
        for _ in range(r.randint(1, 4)):
            pad = "    " * indent
            c = r.choice(["assign", "assign", "show", "if", "for", "try", "push", "aug", "fstr"] + (["return"] if in_func else []))
            if depth > 2 and c in ("if", "for", "try"):
                c = "assign"
            if c == "assign":
                v = r.choice(["a", "b", "c"])
                lines.append(f"{pad}{v} = {self.int_expr(vars)}")
                if v not in vars:
                    vars = vars + [v]
            elif c == "aug" and vars:
                lines.append(f"{pad}{r.choice(vars)} {r.choice(['+=', '-=', '*='])} {self.int_expr(vars)}")
            elif c == "show":
                lines.append(f"{pad}show {self.int_expr(vars)}, \"s\" + str({self.int_expr(vars)})")
            elif c == "fstr":
                lines.append(f"{pad}show f\"<{{{self.int_expr(vars)}}}|{{xs}}>\"")
            elif c == "push":
                lines.append(f"{pad}xs.push({self.int_expr(vars)})")
            elif c == "if":
                lines.append(f"{pad}if {self.bool_expr(vars)}:")
                lines += self.block(vars, indent+1, depth+1, in_func)
                if r.random() < 0.5:
                    lines.append(f"{pad}else:")
                    lines += self.block(vars, indent+1, depth+1, in_func)
            elif c == "for":
                v = r.choice(["i", "j"])
                lines.append(f"{pad}for {v} in range({r.randint(0, 4)}):")
                lines += self.block(vars + [v], indent+1, depth+1, in_func)
            elif c == "try":
                lines.append(f"{pad}try:")
                lines += self.block(vars, indent+1, depth+1, in_func)
                lines.append(f"{pad}catch e:")
                lines.append(f"{pad}    show \"caught\", e")
            elif c == "return":
                lines.append(f"{pad}return {self.int_expr(vars)}")
            else:
                lines.append(f"{pad}show {self.int_expr(vars)}")
        return lines

    def program(self):
        r = self.r
        out = ["xs: List[Int] = [4, 5, 6]"]
        for k in range(r.randint(1, 4)):
            n = r.randint(0, 2)
            params = ["p", "q"][:n]
            name = f"f{k}"
            out.append(f"function {name}({', '.join(p + ': Int' for p in params)}) -> Int:")
            body = self.block(params + ["a", "b", "c"], 1, 0, True)
            out.append("    a = 1\n    b = 2\n    c = 3")
            out += body
            out.append(f"    return {self.int_expr(params + ['a', 'b', 'c'])}")
            self.funcs.append((name, n))
        out.append("a = 0\nb = 1\nc = 2")
        for _ in range(r.randint(2, 6)):
            body = self.block(["a", "b", "c"], 1, 1, False)
            out.append("try:")
            out += body
            out.append("catch e:\n    show \"top caught\", e")
        return "\n".join(out) + "\n"


class MixedPrograms:
    """Floats, strings, maps, closures, queries and a class."""

    def __init__(self, r):
        self.r = r

    def i(self, d=0):
        r = self.r
        c = r.choice(["lit", "var", "bin", "len", "int", "map", "cond", "query", "method", "neg"] if d < 3 else ["lit", "var"])
        if c == "lit": return str(r.choice([0, 1, 2, 3, -4, 10, 255]))
        if c == "var": return r.choice(["n", "m", "k"])
        if c == "bin": return f"({self.i(d+1)} {r.choice(['+', '-', '*', '//', '%'])} {self.i(d+1)})"
        if c == "len": return r.choice(["len(words)", "len(table)", "len(nums)", f"len({self.s(d+1)})"])
        if c == "int": return f"int({self.f(d+1)})"
        if c == "map": return f"table.get({self.s(d+1)}, {self.i(d+1)})"
        if c == "cond": return f"({self.i(d+1)} if {self.b(d+1)} else {self.i(d+1)})"
        if c == "query": return r.choice([f"(nums select it * {self.i(d+1)} sum)", f"len(nums where it > {self.i(d+1)})", "(nums sum)", f"P({self.i(d+1)}, {self.f(d+1)}).scale({self.i(d+1)})", f"add({self.i(d+1)})"])
        if c == "method": return r.choice([f"{self.s(d+1)}.index_of({self.s(d+1)})", "nums.index_of(3)", f"ord({self.s(d+1)}[0])" ])
        return f"-{self.i(d+1)}"

    def f(self, d=0):
        r = self.r
        c = r.choice(["lit", "var", "bin", "conv", "math", "cond", "pow"] if d < 3 else ["lit", "var"])
        if c == "lit": return r.choice(["0.0", "1.5", "-2.25", "1e10", "0.1", "3.0"])
        if c == "var": return r.choice(["x", "y"])
        if c == "bin": return f"({self.f(d+1)} {r.choice(['+', '-', '*', '/'])} {self.f(d+1)})"
        if c == "conv": return f"float({self.i(d+1)})"
        if c == "math": return r.choice([f"math.sqrt({self.f(d+1)})", f"math.floor({self.f(d+1)}) * 1.0", f"abs({self.f(d+1)})", f"round({self.f(d+1)}) * 1.0", f"max({self.f(d+1)}, {self.f(d+1)})"])
        if c == "pow": return f"({self.f(d+1)} ** {r.choice(['2', '0.5', '-1', '3.0'])})"
        return f"({self.f(d+1)} if {self.b(d+1)} else {self.f(d+1)})"

    def s(self, d=0):
        r = self.r
        c = r.choice(["lit", "var", "cat", "slice", "up", "str", "idx", "join", "fstr", "rep"] if d < 3 else ["lit", "var"])
        if c == "lit": return r.choice(['"ab"', '""', '"Hello"', '"ü✓"', '"x y"'])
        if c == "var": return r.choice(["w", "words[0]"])
        if c == "cat": return f"({self.s(d+1)} + {self.s(d+1)})"
        if c == "slice": return f"{self.s(d+1)}[{self.i(d+1)}:{self.i(d+1)}]"
        if c == "up": return f"{self.s(d+1)}.{r.choice(['upper()', 'lower()', 'trim()'])}"
        if c == "str": return f"str({r.choice([self.i(d+1), self.f(d+1)])})"
        if c == "idx": return f"words[{self.i(d+1)}]"
        if c == "join": return f'" ".join(words)'
        if c == "fstr": return f'f"{{{self.i(d+1)}}}-{{{self.f(d+1)}:.2f}}"'
        return f"{self.s(d+1)}.repeat({r.randint(0, 3)})"

    def b(self, d=0):
        r = self.r
        c = r.choice(["icmp", "fcmp", "scmp", "in", "has"])
        if c == "icmp": return f"{self.i(d+1)} {r.choice(['<', '==', '>=', '!='])} {self.i(d+1)}"
        if c == "fcmp": return f"{self.f(d+1)} {r.choice(['<', '>', '<='])} {self.f(d+1)}"
        if c == "scmp": return f"{self.s(d+1)} {r.choice(['==', '<', '!='])} {self.s(d+1)}"
        if c == "in": return f"{self.s(d+1)} in words"
        return f"table.has({self.s(d+1)})"

    def stmt(self, ind, d):
        r = self.r
        p = "    " * ind
        c = r.choice(["n", "x", "w", "show", "push", "set", "if", "for", "try", "pop", "while"])
        if d > 2 and c in ("if", "for", "try", "while"): c = "show"
        if c == "n": return [f"{p}{r.choice(['n', 'm', 'k'])} {r.choice(['=', '+=', '-='])} {self.i()}"]
        if c == "x": return [f"{p}{r.choice(['x', 'y'])} = {self.f()}"]
        if c == "w": return [f"{p}w = {self.s()}"]
        if c == "show": return [f"{p}show {r.choice([self.i(), self.f(), self.s(), self.b()])}, {r.choice([self.i(), self.s()])}"]
        if c == "push": return [f"{p}{r.choice(['nums.push(' + self.i() + ')', 'words.push(' + self.s() + ')'])}"]
        if c == "set": return [f"{p}table[{self.s()}] = {self.i()}"]
        if c == "pop": return [f"{p}show {r.choice(['nums.pop()', 'words.pop()', 'table.remove(' + self.s() + ')'])}"]
        if c == "if":
            out = [f"{p}if {self.b()}:"] + self.block(ind+1, d+1)
            if r.random() < 0.5: out += [f"{p}else:"] + self.block(ind+1, d+1)
            return out
        if c == "for":
            v, q = r.choice([("nums", "qi"), ("words", "qs"), ("table", "qs"), (f"range({r.randint(0,3)})", "qi"), ("w", "qs")])
            return [f"{p}for {q} in {v}:", f"{p}    show {q}"] + self.block(ind+1, d+1)
        if c == "while":
            return [f"{p}guard = 0", f"{p}while guard < {r.randint(0, 3)} and {self.b()}:", f"{p}    guard += 1"] + self.block(ind+1, d+1)
        return [f"{p}try:"] + self.block(ind+1, d+1) + [f"{p}catch e:", f"{p}    show \"caught\", e"]

    def block(self, ind, d):
        out = []
        for _ in range(self.r.randint(1, 3)):
            out += self.stmt(ind, d)
        return out

    def program(self):
        head = ["use math", "",
                "class P:", "    a: Int", "    b: Float", "    function scale(f: Int) -> Int:", "        return self.a * f + int(self.b)", "",
                "nums = [3, 1, 2]", "words = [\"b\", \"a\"]", "table = {\"a\": 1}", "base = 7", "add = |v: Int| v + base",
                "n = 1", "m = 2", "k = 3", "x = 0.5", "y = -1.0", "w = \"w\"", ""]
        body = []
        for _ in range(self.r.randint(3, 7)):
            body += ["try:"] + self.block(1, 1) + ["catch e:", "    show \"top\", e"]
        func = ["function run(n: Int, x: Float, w: String) -> Int:", "    m = 2", "    k = 3", "    y = 1.0"] + self.block(1, 1) + ["    return n + m + k", ""]
        return "\n".join(head + func + body + [f"show run({self.r.randint(-3, 3)}, 1.5, \"z\")"]) + "\n"


PIECES = ["(", ")", "[", "]", "{", "}", ":", ",", ".", "|", "\n", "    ", "if ", "else ", "for ", "in ", "return ",
          "x", "0", "-1", "\"", "f\"{", "}", "#", "@", "**", "Tensor[", "_", "select ", "together ", "await ",
          "function f(", "class C:\n", "=", "+=", "not ", "1.5", "none", "true", "\t", "\\", "'"]


def mutate(src, r):
    for _ in range(r.randint(1, 4)):
        p = r.randrange(len(src) + 1)
        roll = r.random()
        if roll < 0.4:
            src = src[:p] + r.choice(PIECES) + src[p:]
        elif roll < 0.7:
            src = src[:p] + src[min(len(src), p + r.randint(1, 8)):]
        else:
            src = src[:p] + src[p:min(len(src), p + r.randint(1, 30))] + src[p:]
    return src


def cmd_mutate(a):
    sources = [open(f).read() for f in glob.glob(f"{ROOT}/tests/programs/*.nv") + glob.glob(f"{ROOT}/examples/**/*.nv", recursive=True)]
    r = random.Random(a.seed)
    found = 0
    path = os.path.join(a.out, "case.nv")
    for _ in range(a.count):
        src = mutate(r.choice(sources), r)
        open(path, "w").write(src)
        for args in (["check", path], ["fmt", "--check", path]):
            if crashed(run(a.novo, args)):
                found += 1
                open(os.path.join(a.out, f"crash_{found}.nv"), "w").write(src)
                print(f"crash in `novo {args[0]}`: saved crash_{found}.nv")
    print(f"mutate: {a.count} programs, {found} crashes")
    return found


def engines_differ(novo, path):
    native, interp = run(novo, ["run", path]), run(novo, ["run", "--interp", path])
    if "timeout" in (native[0], interp[0]):
        return False
    return native != interp or crashed(native) or crashed(interp)


def cmd_diff(a):
    found = checked = 0
    path = os.path.join(a.out, "case.nv")
    for i in range(a.count):
        seed = a.seed * 1_000_000 + i
        kind = IntPrograms if i % 2 == 0 else MixedPrograms
        src = kind(random.Random(seed)).program()
        open(path, "w").write(src)
        code, _, err = run(a.novo, ["check", path])
        if "panicked" in err:
            found += 1
            open(os.path.join(a.out, f"check_panic_{seed}.nv"), "w").write(src)
            print(f"checker panic: saved check_panic_{seed}.nv")
        if code != 0:
            continue
        checked += 1
        if engines_differ(a.novo, path):
            found += 1
            open(os.path.join(a.out, f"differ_{seed}.nv"), "w").write(src)
            print(f"engines differ: saved differ_{seed}.nv")
    print(f"diff: {checked} of {a.count} programs ran, {found} failures")
    return found


def cmd_reduce(a):
    lines = open(a.file).read().splitlines()
    path = a.file + ".reduced.nv"

    def failing(candidate):
        open(path, "w").write("\n".join(candidate) + "\n")
        return run(a.novo, ["check", path])[0] == 0 and engines_differ(a.novo, path)

    def indent(line):
        return len(line) - len(line.lstrip())

    if not failing(lines):
        print("the engines agree on this program")
        return 0
    changed = True
    while changed:
        changed = False
        i = 0
        while i < len(lines):
            end = i + 1
            while end < len(lines) and lines[end].strip() and indent(lines[end]) > indent(lines[i]):
                end += 1
            for candidate in (lines[:i] + lines[end:], lines[:i] + lines[i + 1:]):
                if len(candidate) < len(lines) and failing(candidate):
                    lines, changed = candidate, True
                    break
            else:
                i += 1
    failing(lines)
    print("\n".join(lines))
    print(f"\nsaved {path}", file=sys.stderr)
    return 0


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("mode", choices=["mutate", "diff", "reduce"])
    p.add_argument("file", nargs="?")
    p.add_argument("--count", type=int, default=200)
    p.add_argument("--seed", type=int, default=0)
    p.add_argument("--novo", default=os.path.join(ROOT, "target", "release", "novo"))
    p.add_argument("--out", default=os.path.join(ROOT, "target", "fuzz"))
    a = p.parse_args()
    os.makedirs(a.out, exist_ok=True)
    if a.mode == "reduce" and not a.file:
        p.error("reduce needs a file")
    found = {"mutate": cmd_mutate, "diff": cmd_diff, "reduce": cmd_reduce}[a.mode](a)
    sys.exit(1 if found else 0)


if __name__ == "__main__":
    main()
