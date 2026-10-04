import time
import json

start_time = time.time()

with open("data.json", "r") as f:
    data = json.load(f)

total = sum(item["stats"]["score"] for item in data if item["active"])

end_time = time.time()
print(total)
print(end_time - start_time)
