import time

start_time = time.time()

with open("in.txt", "r") as f:
    lines = f.readlines()
    
out = [line for line in lines if "error" in line]

with open("out.txt", "w") as f:
    f.writelines(out)
    
end_time = time.time()
print(end_time - start_time)
