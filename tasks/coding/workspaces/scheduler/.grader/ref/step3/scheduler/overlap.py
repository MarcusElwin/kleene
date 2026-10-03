"""Overlap checks and busy totals."""

from scheduler.intervals import merge, parse_time


def overlaps(a, b):
    """Whether two (start, end) intervals share any minute; intervals that
    only touch do not overlap."""
    return a[0] < b[1] and b[0] < a[1]


def total_busy(intervals):
    """Minutes covered by the intervals, each minute counted once."""
    return sum(end - start for start, end in merge(intervals))


def parse_span(text):
    """'HH:MM-HH:MM' to (start, end) minutes, where a span that ends at or
    before its start runs past midnight into the next day (end + 1440)."""
    parts = text.split("-")
    if len(parts) != 2:
        raise ValueError(f"bad span {text!r}")
    start, end = parse_time(parts[0]), parse_time(parts[1])
    if end <= start:
        end += 1440
    return (start, end)
