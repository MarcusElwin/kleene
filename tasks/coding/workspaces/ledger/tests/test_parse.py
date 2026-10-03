import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

from ledger.parse import parse_transactions


class ParseTests(unittest.TestCase):
    def test_basic_rows(self):
        rows = parse_transactions("date,description,amount,currency\n2026-01-03,Groceries,84.20,USD\n")
        self.assertEqual(rows, [{"date": "2026-01-03", "description": "Groceries", "amount": 84.2, "currency": "USD"}])

    def test_parentheses_and_commas_mean_negative_thousands(self):
        rows = parse_transactions('date,description,amount,currency\n2026-01-15,Rent,"(1,200.00)",USD\n')
        self.assertEqual(rows[0]["amount"], -1200.0)

    def test_blank_and_comment_lines_are_skipped(self):
        text = "# export\n\ndate,description,amount,currency\n\n2026-02-01,Books,-35.50,GBP\n"
        rows = parse_transactions(text)
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["amount"], -35.5)

    def test_missing_currency_is_usd(self):
        rows = parse_transactions("date,description,amount,currency\n2026-02-02,Refund,12.30,\n")
        self.assertEqual(rows[0]["currency"], "USD")


if __name__ == "__main__":
    unittest.main()
