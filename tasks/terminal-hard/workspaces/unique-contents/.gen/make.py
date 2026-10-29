import os, random
r = random.Random(13)
contents = [("\n".join(f"line {r.randrange(10**6)}" for _ in range(r.randint(3, 40))) + "\n") for _ in range(137)]
dirs = ["data", "data/a", "data/b", "data/b/old", "data/c", "data/c/2025", "data/c/2026"]
for d in dirs:
    os.makedirs(d, exist_ok=True)
for i in range(420):
    c = contents[r.randrange(len(contents))] if i >= 137 else contents[i]
    with open(f"{r.choice(dirs)}/file_{i:04}.{r.choice(['txt','csv','md','log'])}", "w") as f:
        f.write(c)
