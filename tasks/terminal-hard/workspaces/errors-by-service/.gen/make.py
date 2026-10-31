import os, random
r = random.Random(11)
services = ["auth", "billing", "catalog", "gateway", "search", "shipping"]
msgs = ["request completed in {n} ms", "cache miss for key k{n}", "retrying upstream call ({n})",
        "recovered from ERROR state after {n} s", "connection pool at {n}%", "user {n} logged in",
        "ERROR budget consumed: {n}%", "scheduled job {n} finished"]
for s in services:
    os.makedirs(f"logs/{s}", exist_ok=True)
    for day in range(1, 13):
        with open(f"logs/{s}/2026-03-{day:02}.log", "w") as f:
            for i in range(r.randint(150, 400)):
                lvl = r.choices(["DEBUG", "INFO", "WARN", "ERROR"], [3, 6, 2, 1])[0]
                ts = f"2026-03-{day:02}T{r.randint(0,23):02}:{r.randint(0,59):02}:{r.randint(0,59):02}Z"
                f.write(f"{ts} {lvl} {s} {r.choice(msgs).format(n=r.randint(1, 999))}\n")
