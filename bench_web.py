import time
import urllib.request
import re

start_time = time.time()

response = urllib.request.urlopen("https://news.ycombinator.com")
html = response.read().decode('utf-8')
links = re.findall('href="(http[^"]+)"', html)
secure = [link for link in links if link.startswith("https")]

end_time = time.time()
print(end_time - start_time)
