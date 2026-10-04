import urllib.request
import time

start = time.time()
url = "https://example.com"
response = urllib.request.urlopen(url)
html = response.read().decode('utf-8')
title_parts = html.split("<title>")
if len(title_parts) > 1:
    title = title_parts[1].split("</title>")[0]
    print("Title is:")
    print(title)
else:
    print("No title found")
end = time.time()
print(end - start)
