import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

import subprocess


def run(*args):
    return subprocess.run([sys.executable, "-m", "ledger", *args], capture_output=True, text=True, cwd=os.getcwd())


class CliTests(unittest.TestCase):
    def test_totals_are_converted_and_formatted(self):
        r = run("data/transactions.csv", "--rates", "data/rates.json")
        self.assertEqual(r.returncode, 0, r.stderr)
        lines = r.stdout.strip().splitlines()
        # January: -84.20 + 3200 - 62*1.08 - 1200 = 1848.84
        # February: -36.00*1.27 + 12.30 - 120 + 450*1.08 = 332.58
        self.assertEqual(lines, ["2026-01\t1848.84", "2026-02\t332.58"])

    def test_base_currency(self):
        r = run("data/transactions.csv", "--rates", "data/rates.json", "--base", "EUR", "--month", "2026-01")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stdout.strip(), "2026-01\t1711.89")

    def test_two_decimals_always(self):
        with open("tmp_two.csv", "w") as f:
            f.write("date,description,amount,currency\n2026-05-01,a,0.10,USD\n2026-05-02,b,0.20,USD\n")
        try:
            r = run("tmp_two.csv", "--rates", "data/rates.json")
        finally:
            os.remove("tmp_two.csv")
        self.assertEqual(r.stdout.strip(), "2026-05\t0.30")

    def test_missing_file_is_a_clean_error(self):
        r = run("does-not-exist.csv", "--rates", "data/rates.json")
        self.assertEqual(r.returncode, 2)
        self.assertTrue(r.stderr.startswith("error:"), r.stderr)
        self.assertNotIn("Traceback", r.stderr)

    def test_unknown_currency_is_a_clean_error(self):
        with open("tmp_cur.csv", "w") as f:
            f.write("date,description,amount,currency\n2026-05-01,a,1.00,CHF\n")
        try:
            r = run("tmp_cur.csv", "--rates", "data/rates.json")
        finally:
            os.remove("tmp_cur.csv")
        self.assertEqual(r.returncode, 2)
        self.assertIn("CHF", r.stderr)
        self.assertNotIn("Traceback", r.stderr)


if __name__ == "__main__":
    unittest.main()
