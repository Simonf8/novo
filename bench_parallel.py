import time
import concurrent.futures

def process_item(item):
    time.sleep(1)
    return item * 2

start = time.time()
with concurrent.futures.ThreadPoolExecutor() as executor:
    results = list(executor.map(process_item, [1, 2, 3, 4, 5]))
end = time.time()
print(end - start)
