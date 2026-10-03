"""Transaction CSV parsing."""

import csv
import io
import re

REQUIRED = ("date", "description", "amount", "currency")


def _parse_amount(raw, line_no):
    s = raw.strip()
    negative = False
    if s.startswith("(") and s.endswith(")"):
        negative = True
        s = s[1:-1].strip()
    s = s.replace(",", "")
    if s.startswith("-"):
        negative = not negative
        s = s[1:]
    if not re.fullmatch(r"\d+(\.\d+)?", s):
        raise ValueError(f"line {line_no}: bad amount {raw!r}")
    value = round(float(s), 2)
    return -value if negative else value


def _parse_date(raw, line_no):
    s = raw.strip()
    if re.fullmatch(r"\d{4}-\d{2}-\d{2}", s):
        return s
    m = re.fullmatch(r"(\d{2})/(\d{2})/(\d{4})", s)
    if m:
        return f"{m.group(3)}-{m.group(2)}-{m.group(1)}"
    raise ValueError(f"line {line_no}: bad date {raw!r}")


def parse_transactions(text):
    """Parse CSV text into a list of transaction dicts.

    Blank lines and lines starting with '#' are skipped. The first
    remaining line is the header; columns may come in any order. Amounts
    may carry thousands separators and parentheses for negatives; dates are
    ISO or DD/MM/YYYY and are returned as ISO. A missing currency is USD.
    A bad amount or date raises ValueError naming the 1-based line number.
    """
    rows = []
    header = None
    for line_no, line in enumerate(text.splitlines(), start=1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        cells = next(csv.reader(io.StringIO(line)))
        if header is None:
            header = [c.strip().lower() for c in cells]
            missing = [c for c in REQUIRED if c not in header]
            if missing:
                raise ValueError(f"line {line_no}: missing columns {missing}")
            continue
        record = {name: (cells[i] if i < len(cells) else "") for i, name in enumerate(header)}
        currency = record["currency"].strip().upper() or "USD"
        rows.append(
            {
                "date": _parse_date(record["date"], line_no),
                "description": record["description"].strip(),
                "amount": _parse_amount(record["amount"], line_no),
                "currency": currency,
            }
        )
    return rows
