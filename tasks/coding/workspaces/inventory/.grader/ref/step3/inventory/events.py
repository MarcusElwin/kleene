"""Replay an event log into an Inventory, and stock reports."""

import json


def low_stock(inv, threshold):
    """(sku, available) pairs for skus with stock on hand whose available
    quantity is strictly below the threshold, least available first, ties
    by sku."""
    rows = [(s, inv.available(s)) for s in inv.skus() if inv.available(s) < threshold]
    rows.sort(key=lambda r: (r[1], r[0]))
    return rows


def apply_events(inv, lines):
    """Apply JSON-lines events to an inventory and return how many were applied.

    Each line is an object with id, ts, type (add, remove, reserve, release,
    fulfil), and sku, qty and order as the type needs. Events are applied in
    order of ts (ties in the order given), and an id seen before is skipped.
    Blank lines are ignored.
    """
    events = []
    for i, line in enumerate(lines):
        if not line.strip():
            continue
        e = json.loads(line)
        events.append((e["ts"], i, e))
    events.sort(key=lambda x: (x[0], x[1]))
    seen = set()
    applied = 0
    for _, _, e in events:
        if e["id"] in seen:
            continue
        seen.add(e["id"])
        t = e["type"]
        if t == "add":
            inv.add(e["sku"], e["qty"])
        elif t == "remove":
            inv.remove(e["sku"], e["qty"])
        elif t == "reserve":
            inv.reserve(e["order"], e["sku"], e["qty"])
        elif t == "release":
            inv.release(e["order"])
        elif t == "fulfil":
            inv.fulfil(e["order"])
        else:
            raise ValueError(f"unknown event type {t!r}")
        applied += 1
    return applied
