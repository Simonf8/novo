import time

start_time = time.time()
total = 0
for _ in range(10000000):
    total = total + 1
end_time = time.time()
elapsed = end_time - start_time
print(total)
print("Python Elapsed (seconds):")
print(elapsed)
