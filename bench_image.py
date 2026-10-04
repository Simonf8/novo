import time
import cv2

start_time = time.time()

img = cv2.imread("input.jpg")
gray = cv2.cvtColor(img, cv2.COLOR_BGR2GRAY)
blurred = cv2.GaussianBlur(gray, (5, 5), 0)
cv2.imwrite("output.jpg", blurred)

end_time = time.time()
print(end_time - start_time)
