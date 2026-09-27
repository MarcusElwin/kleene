"""Command line: monthly totals in one base currency.

usage: python3 -m ledger <transactions.csv> --rates <rates.json> [--base USD] [--month YYYY-MM]

Prints one line per month, "YYYY-MM<TAB>total", totals in the base currency.
"""

import argparse
import json
import sys

from ledger.parse import parse_transactions


def convert(amount, currency, base, rates):
    # rates map a currency to its value in USD (USD = 1.0)
    return amount / rates[currency] * rates[base]


def main(argv=None):
    ap = argparse.ArgumentParser(prog="ledger")
    ap.add_argument("csv")
    ap.add_argument("--rates", required=True)
    ap.add_argument("--base", default="USD")
    ap.add_argument("--month")
    args = ap.parse_args(argv)
    rows = parse_transactions(open(args.csv).read())
    rates = json.load(open(args.rates))
    totals = {}
    for t in rows:
        month = t["date"][:7]
        if args.month and month != args.month:
            continue
        totals[month] = totals.get(month, 0.0) + convert(t["amount"], t["currency"], args.base, rates)
    for month in sorted(totals):
        print(f"{month}\t{totals[month]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
