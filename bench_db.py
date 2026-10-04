import time
import sqlite3

start = time.time()

conn = sqlite3.connect("test.db")
c = conn.cursor()
c.execute("DROP TABLE IF EXISTS users")
c.execute("CREATE TABLE users (id INTEGER, name TEXT)")

for i in range(100):
    c.execute("INSERT INTO users VALUES (?, ?)", (i, f"User {i}"))

c.execute("SELECT * FROM users")
rows = c.fetchall()
conn.commit()
conn.close()

end = time.time()
print(end - start)
