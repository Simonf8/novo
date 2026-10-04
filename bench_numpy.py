import time
import numpy as np

start_time = time.time()

matrix = np.random.rand(1000, 1000)
scaled = matrix * 5.0
mean_val = np.mean(scaled)

end_time = time.time()
print(end_time - start_time)
