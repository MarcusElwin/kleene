import csv, os, random
r = random.Random(14)
os.makedirs("assets", exist_ok=True)
cats = ["icons", "photos", "docs", "audio"]
with open("manifest.csv", "w", newline="") as f:
    w = csv.writer(f); w.writerow(["old", "new"])
    for i in range(1, 151):
        old = f"assets/img_{i:04}.bin"
        open(old, "w").write(f"asset {i}\n")
        w.writerow([old, f"renamed/{r.choice(cats)}/{r.randrange(2020, 2027)}/{r.choice(['a','b','c'])}_{i:04}.bin"])
