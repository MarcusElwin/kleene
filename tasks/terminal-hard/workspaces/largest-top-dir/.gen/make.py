import os, random
r = random.Random(15)
tops = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel"]
for t in tops:
    for i in range(r.randint(30, 80)):
        depth = r.randint(0, 3)
        d = os.path.join("tree", t, *[f"d{r.randrange(5)}" for _ in range(depth)])
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, f"f{i:03}.dat"), "wb") as f:
            f.write(b"x" * r.randint(100, 60000))
