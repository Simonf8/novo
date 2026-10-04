import numpy as np
from sklearn.linear_model import LogisticRegression
import time

start = time.time()
X = np.array([[1, 2], [2, 3], [3, 4], [4, 5]])
y = np.array([0, 0, 1, 1])

model = LogisticRegression()
model.fit(X, y)

test_data = np.array([[1.5, 2.5], [3.5, 4.5]])
predictions = model.predict(test_data)
end = time.time()
print(end - start)
