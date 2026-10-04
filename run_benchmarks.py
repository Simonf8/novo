import os
import subprocess
import sys
import time
import statistics

# Set of benchmarks to run
BENCHMARKS = [
    ("1000_line", "bench_1000.py", "bench_1000.nv"),
    ("CSV", "bench_csv.py", "bench_csv.nv"),
    ("JSON", "bench_json.py", "bench_json.nv"),
    ("Scraper", "bench_web.py", "bench_scraper.nv"),
    ("Image", "bench_image.py", "bench_image.nv"),
    ("DB", "bench_db.py", "bench_db.nv"),
    ("Robotics", "bench_sensor.py", "bench_robotics.nv"),
    ("File", "bench_file.py", "bench_file.nv"),
    ("ML", "bench_ml.py", "bench_ml.nv")
]

ITERATIONS = 10

def run_cmd(cmd):
    start = time.time()
    try:
        # We need to run cargo so it builds if needed, or just novo.exe
        # To avoid cargo overhead in timing, we time only the execution itself.
        # Wait, the script execution itself might print the time if it's internal.
        # Let's rely on the internal time printing!
        # Both Python and Novo scripts print the elapsed time as the last line.
        env = os.environ.copy()
        env["PATH"] = os.path.dirname(sys.executable) + os.pathsep + env.get("PATH", "")
        result = subprocess.run(cmd, capture_output=True, text=True, check=True, shell=True, env=env)
        # Parse last line for time
        lines = result.stdout.strip().split("\n")
        last_line = lines[-1].strip()
        # Some scripts might have "Novo Elapsed (seconds):" before it.
        # Let's just find the last float.
        for line in reversed(lines):
            try:
                val = float(line.strip())
                return val
            except ValueError:
                pass
        return None
    except subprocess.CalledProcessError as e:
        print(f"Error running {' '.join(cmd)}:\n{e.stderr}")
        return None
    except Exception as e:
        print(f"Error: {e}")
        return None

def main():
    print("# Performance Benchmark Results\n")
    print("| Benchmark | Language | Mean (s) | Median (s) | Min (s) | Max (s) | StdDev (s) |")
    print("| :--- | :--- | :--- | :--- | :--- | :--- | :--- |")
    
    # Pre-build novo
    print("Building Novo...")
    subprocess.run([r"C:\Users\kefet\.cargo\bin\cargo", "build"], check=True, capture_output=True, shell=True)
    novo_exe = os.path.join("target", "debug", "novo.exe")
    
    for name, py_file, nv_file in BENCHMARKS:
        # Run Python
        py_times = []
        if os.path.exists(py_file):
            for i in range(ITERATIONS):
                t = run_cmd(["python", py_file])
                if t is not None:
                    py_times.append(t)
        
        # Run Novo
        nv_times = []
        if os.path.exists(nv_file):
            for i in range(ITERATIONS):
                t = run_cmd([novo_exe, nv_file])
                if t is not None:
                    nv_times.append(t)
        
        def format_stats(times, lang):
            if not times:
                return f"| {name} | {lang} | N/A | N/A | N/A | N/A | N/A |"
            mean = statistics.mean(times)
            median = statistics.median(times)
            min_t = min(times)
            max_t = max(times)
            stddev = statistics.stdev(times) if len(times) > 1 else 0.0
            return f"| {name} | {lang} | {mean:.4f} | {median:.4f} | {min_t:.4f} | {max_t:.4f} | {stddev:.4f} |"

        print(format_stats(py_times, "Python"))
        print(format_stats(nv_times, "Novo"))

if __name__ == "__main__":
    main()
