import cv2
import numpy as np

img = np.ones((100, 100, 3), dtype=np.uint8) * 255
cv2.imwrite("input.jpg", img)
