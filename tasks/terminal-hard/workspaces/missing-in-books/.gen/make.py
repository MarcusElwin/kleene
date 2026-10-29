import csv, random
r = random.Random(17)
rows = [(f"T{100000 + i}", f"2026-{r.randint(1,9):02}-{r.randint(1,28):02}", f"{r.uniform(-500, 2000):.2f}") for i in range(2500)]
missing = set(r.sample(range(2500), 17))
extra = [(f"T{200000 + i}", "2026-05-05", f"{r.uniform(1, 100):.2f}") for i in range(9)]
with open("bank.csv", "w", newline="") as f:
    w = csv.writer(f); w.writerow(["id", "date", "amount"]); w.writerows(rows)
books = [row for i, row in enumerate(rows) if i not in missing] + extra
r.shuffle(books)
with open("books.csv", "w", newline="") as f:
    w = csv.writer(f); w.writerow(["id", "date", "amount"]); w.writerows(books)
