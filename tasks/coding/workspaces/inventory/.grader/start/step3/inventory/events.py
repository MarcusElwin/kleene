"""Replay an event log into an Inventory, and stock reports."""

import json


def low_stock(inv, threshold):
    """Skus whose available quantity is below the threshold."""
    return [s for s in inv.skus() if inv.available(s) <= threshold]


def apply_events(inv, lines):
    """Apply JSON-lines events to an inventory and return how many were applied.

    Each line is an object with id, ts, type (add, remove, reserve, release,
    fulfil), and sku, qty and order as the type needs.
    """
    applied = 0
    for line in lines:
        if not line.strip():
            continue
        e = json.loads(line)
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
