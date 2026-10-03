"""Command line: monthly totals in one base currency.

usage: python3 -m ledger <transactions.csv> --rates <rates.json> [--base USD] [--month YYYY-MM]

Prints one line per month, "YYYY-MM<TAB>total", totals in the base currency
with two decimals. A missing or unreadable file, or an unknown currency,
prints "error: ..." on stderr and exits with status 2.
"""

import argparse
import json
import sys

from ledger.parse import parse_transactions


def convert(amount, currency, base, rates):
    # rates map a currency to its value in USD (USD = 1.0)
    return amount * rates[currency] / rates[base]


def main(argv=None):
    ap = argparse.ArgumentParser(prog="ledger")
    ap.add_argument("csv")
    ap.add_argument("--rates", required=True)
    ap.add_argument("--base", default="USD")
    ap.add_argument("--month")
    args = ap.parse_args(argv)
    try:
        with open(args.csv) as f:
            rows = parse_transactions(f.read())
        with open(args.rates) as f:
            rates = json.load(f)
        totals = {}
        for t in rows:
            month = t["date"][:7]
            if args.month and month != args.month:
                continue
            if t["currency"] not in rates or args.base not in rates:
                raise KeyError(t["currency"] if t["currency"] not in rates else args.base)
            totals[month] = totals.get(month, 0.0) + convert(t["amount"], t["currency"], args.base, rates)
    except (OSError, ValueError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    except KeyError as e:
        print(f"error: unknown currency {e.args[0]}", file=sys.stderr)
        return 2
    for month in sorted(totals):
        print(f"{month}\t{totals[month]:.2f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
