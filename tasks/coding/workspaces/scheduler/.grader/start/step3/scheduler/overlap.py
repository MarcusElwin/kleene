"""Overlap checks and busy totals."""

from scheduler.intervals import parse_time


def overlaps(a, b):
    """Whether two (start, end) intervals share any minute."""
    return a[0] <= b[1] and b[0] <= a[1]


def total_busy(intervals):
    """Minutes covered by the intervals."""
    return sum(end - start for start, end in intervals)


def parse_span(text):
    """'HH:MM-HH:MM' to (start, end) minutes, where a span that ends before
    it starts runs past midnight."""
    start_text, end_text = text.split("-")
    start, end = parse_time(start_text), parse_time(end_text)
    if end <= start:
        raise ValueError(f"bad span {text!r}")
    return (start, end)
