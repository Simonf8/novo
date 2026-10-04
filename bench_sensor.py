import time
import math

class SensorData:
    def __init__(self, x, y, z):
        self.x = x
        self.y = y
        self.z = z

def read_sensor():
    return [SensorData(1.0, 2.0, 3.0), SensorData(100.0, 200.0, 300.0), SensorData(1.5, 2.5, 3.5)]

start = time.time()
data = read_sensor()

valid_magnitudes = []
for d in data:
    mag = math.sqrt(d.x**2 + d.y**2 + d.z**2)
    if mag < 10.0:
        valid_magnitudes.append(mag)

end = time.time()
print(end - start)
