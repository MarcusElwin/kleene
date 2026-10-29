import csv, random
r = random.Random(12)
countries = ["Sweden", "Norway", "Denmark", "Finland", "Germany", "Netherlands", "Poland", "Estonia", "France", "Spain"]
with open("customers.csv", "w", newline="") as f:
    w = csv.writer(f); w.writerow(["customer_id", "name", "country"])
    for i in range(300):
        w.writerow([f"C{i:04}", f"Customer {i}", r.choice(countries)])
with open("orders.csv", "w", newline="") as f:
    w = csv.writer(f); w.writerow(["order_id", "customer_id", "amount", "status"])
    for i in range(4000):
        w.writerow([f"O{i:05}", f"C{r.randrange(300):04}", f"{r.uniform(5, 900):.2f}",
                    r.choices(["paid", "refunded", "pending", "cancelled", "PAID "], [7, 1, 1, 1, 1])[0]])
