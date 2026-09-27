import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

from ledger.report import monthly_totals, top_expenses, balance_by_currency


def tx(date, desc, amount, cur="USD"):
    return {"date": date, "description": desc, "amount": amount, "currency": cur}


class ReportTests(unittest.TestCase):
    def test_monthly_totals(self):
        rows = [tx("2026-01-03", "a", -10.0), tx("2026-01-20", "b", 25.5), tx("2026-03-01", "c", 1.0)]
        self.assertEqual(monthly_totals(rows), {"2026-01": 15.5, "2026-03": 1.0})

    def test_top_expenses_largest_outflow_first(self):
        rows = [tx("2026-01-03", "a", -10.0), tx("2026-01-04", "b", -50.0), tx("2026-01-05", "c", 5.0)]
        self.assertEqual([t["description"] for t in top_expenses(rows, 2)], ["b", "a"])

    def test_balance_by_currency(self):
        rows = [tx("2026-01-03", "a", -10.0), tx("2026-01-04", "b", 4.0, "EUR"), tx("2026-01-05", "c", 6.0)]
        self.assertEqual(balance_by_currency(rows), {"EUR": 4.0, "USD": -4.0})


if __name__ == "__main__":
    unittest.main()
