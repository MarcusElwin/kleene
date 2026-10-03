"""Summaries over parsed transactions."""


def monthly_totals(transactions):
    """Sum of amounts per 'YYYY-MM', rounded to cents; months absent when empty."""
    totals = {}
    for t in transactions:
        key = t["date"][:7]
        totals[key] = totals.get(key, 0.0) + t["amount"]
    return {k: round(v, 2) for k, v in sorted(totals.items())}


def top_expenses(transactions, n):
    """The n most negative transactions: largest outflow first, ties by
    earlier date then description; fewer than n returns them all."""
    expenses = [t for t in transactions if t["amount"] < 0]
    expenses.sort(key=lambda t: (t["amount"], t["date"], t["description"]))
    return expenses[: max(n, 0)]


def balance_by_currency(transactions):
    """Net amount per currency, rounded to cents."""
    totals = {}
    for t in transactions:
        totals[t["currency"]] = totals.get(t["currency"], 0.0) + t["amount"]
    return {k: round(v, 2) for k, v in sorted(totals.items())}
