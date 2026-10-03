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

    def test_dd_mm_yyyy_dates_are_normalised(self):
        rows = parse_transactions("date,description,amount,currency\n02/03/2026,Refund,12.30,USD\n")
        self.assertEqual(rows[0]["date"], "2026-03-02")

    def test_quoted_description_with_comma(self):
        rows = parse_transactions('date,description,amount,currency\n2026-01-09,"Dinner, with friends",(62.00),EUR\n')
        self.assertEqual(rows[0]["description"], "Dinner, with friends")
        self.assertEqual(rows[0]["amount"], -62.0)

    def test_columns_in_any_order(self):
        rows = parse_transactions("amount,currency,date,description\n5.00,eur,2026-01-01,Coffee\n")
        self.assertEqual(rows[0], {"date": "2026-01-01", "description": "Coffee", "amount": 5.0, "currency": "EUR"})

    def test_bad_amount_names_the_line(self):
        text = "# c\ndate,description,amount,currency\n2026-01-01,Ok,1.00,USD\n2026-01-02,Bad,abc,USD\n"
        with self.assertRaises(ValueError) as cm:
            parse_transactions(text)
        self.assertIn("4", str(cm.exception))

    def test_sample_file_parses(self):
        with open("data/transactions.csv") as f:
            rows = parse_transactions(f.read())
        self.assertEqual(len(rows), 8)
        self.assertEqual(sum(1 for r in rows if r["currency"] == "USD"), 5)


if __name__ == "__main__":
    unittest.main()
