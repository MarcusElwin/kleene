import json, random
r = random.Random(18)
with open("events.jsonl", "w") as f:
    for i in range(15000):
        m = r.choices(range(1, 8), [1, 2, 3, 4, 4, 3, 2])[0]
        u = f"u{int(r.paretovariate(1.2) * 10) % 2500:04}"
        f.write(json.dumps({"user": u, "ts": f"2026-{m:02}-{r.randint(1,28):02}T{r.randint(0,23):02}:{r.randint(0,59):02}:00Z",
                            "type": r.choice(["view", "click", "purchase"])}) + "\n")
