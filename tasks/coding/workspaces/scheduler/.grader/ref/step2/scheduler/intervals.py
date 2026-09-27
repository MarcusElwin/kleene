"""Intervals within a day, in minutes from midnight."""

import re


def parse_time(text):
    """'HH:MM' to minutes from midnight; 00:00 to 24:00 inclusive."""
    m = re.fullmatch(r"\s*(\d{1,2}):(\d{2})\s*", text or "")
    if not m:
        raise ValueError(f"bad time {text!r}")
    h, mm = int(m.group(1)), int(m.group(2))
    if mm > 59 or h > 24 or (h == 24 and mm != 0):
        raise ValueError(f"bad time {text!r}")
    return h * 60 + mm


def parse_interval(text):
    """'HH:MM-HH:MM' to (start, end) minutes; the end must be after the start."""
    parts = (text or "").split("-")
    if len(parts) != 2:
        raise ValueError(f"bad interval {text!r}")
    start, end = parse_time(parts[0]), parse_time(parts[1])
    if end <= start:
        raise ValueError(f"bad interval {text!r}: end is not after start")
    return (start, end)


def merge(intervals):
    """Sorted, non-overlapping intervals covering the same minutes; touching
    intervals ([a, b) and [b, c)) are joined."""
    out = []
    for start, end in sorted(intervals):
        if out and start <= out[-1][1]:
            out[-1] = (out[-1][0], max(out[-1][1], end))
        else:
            out.append((start, end))
    return out


def format_interval(interval):
    """(start, end) minutes back to 'HH:MM-HH:MM'."""
    start, end = interval
    return f"{start // 60:02d}:{start % 60:02d}-{end // 60:02d}:{end % 60:02d}"


def free_slots(busy, day_start, day_end, min_len):
    """Gaps of at least min_len minutes between day_start and day_end not
    covered by any busy interval; busy may be unsorted or overlapping and may
    reach outside the day."""
    if day_end <= day_start:
        return []
    out = []
    cursor = day_start
    for start, end in merge(busy):
        if end <= day_start or start >= day_end:
            continue
        start = max(start, day_start)
        if start - cursor >= min_len:
            out.append((cursor, start))
        cursor = max(cursor, min(end, day_end))
    if day_end - cursor >= min_len:
        out.append((cursor, day_end))
    return out
