import time
import csv

start_time = time.time()

with open("users_large.csv", "r", encoding="utf-8") as f:
    reader = csv.DictReader(f)
    data = list(reader)

adults = [item for item in data if int(item['age']) >= 18]
salaries = [int(item['salary']) for item in adults]
avg = sum(salaries) / len(salaries) if salaries else 0

end_time = time.time()
print(avg)
print(end_time - start_time)
