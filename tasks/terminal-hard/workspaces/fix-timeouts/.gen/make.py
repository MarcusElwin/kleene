import json, os, random
r = random.Random(16)
os.makedirs("services", exist_ok=True)
for i in range(60):
    t = 10 + (i * 7) % 50
    cfg = {"name": f"svc-{i:03}", "replicas": r.randint(1, 5), "timeout": str(t) if i % 3 == 0 else t,
           "timeout_ms": str(t * 1000), "env": {"REGION": r.choice(["eu-north-1", "us-east-1"]), "TIMEOUT": str(t)}}
    json.dump(cfg, open(f"services/svc_{i:03}.json", "w"), indent=2)
