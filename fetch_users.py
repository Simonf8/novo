import time
import json
import urllib.request
from concurrent.futures import ThreadPoolExecutor

start_time = time.time()
print("Fetching user data in parallel...")

urls = [
    "https://jsonplaceholder.typicode.com/users/1",
    "https://jsonplaceholder.typicode.com/users/2",
    "https://jsonplaceholder.typicode.com/users/3"
]

def fetch_url(url):
    response = urllib.request.urlopen(url)
    return response.read().decode('utf-8')

with ThreadPoolExecutor(max_workers=3) as executor:
    results = list(executor.map(fetch_url, urls))

print("Parsing JSON...")
users = [json.loads(item) for item in results]
names = [user["name"] for user in users]

end_time = time.time()
elapsed = end_time - start_time

print("Fetched Users:")
print(names)
print("Elapsed time:")
print(elapsed)
