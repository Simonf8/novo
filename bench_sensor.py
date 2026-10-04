import time
import random

start = time.time()
readings = []
for _ in range(10):
    val = random.random() * 100
    readings.append(val)
    time.sleep(0.01)

avg = sum(readings) / len(readings)
max_val = max(readings)

print("Sensor Readings Average:")
print(avg)
print("Max Peak:")
print(max_val)

end = time.time()
print(end - start)
