import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

import json

from inventory.store import Inventory
from inventory.events import low_stock, apply_events


def ev(id, ts, type, **kw):
    return json.dumps(dict(id=id, ts=ts, type=type, **kw))


class ReportTests(unittest.TestCase):
    def test_low_stock_is_strict_and_sorted(self):
        inv = Inventory()
        inv.add("bolt", 5)
        inv.add("nut", 2)
        inv.add("washer", 2)
        inv.add("screw", 9)
        inv.reserve("o1", "bolt", 4)
        self.assertEqual(low_stock(inv, 5), [("bolt", 1), ("nut", 2), ("washer", 2)])
        self.assertEqual(low_stock(inv, 2), [("bolt", 1)])
        self.assertEqual(low_stock(inv, 1), [])


class EventTests(unittest.TestCase):
    def test_events_apply_in_timestamp_order(self):
        inv = Inventory()
        lines = [
            ev("e2", "2026-01-02T10:00:00", "remove", sku="bolt", qty=3),
            ev("e1", "2026-01-01T10:00:00", "add", sku="bolt", qty=5),
        ]
        self.assertEqual(apply_events(inv, lines), 2)
        self.assertEqual(inv.stock("bolt"), 2)

    def test_duplicate_ids_apply_once(self):
        inv = Inventory()
        lines = [
            ev("e1", "2026-01-01T10:00:00", "add", sku="bolt", qty=5),
            ev("e1", "2026-01-01T10:00:00", "add", sku="bolt", qty=5),
            "",
            ev("e2", "2026-01-01T11:00:00", "reserve", order="o1", sku="bolt", qty=2),
            ev("e2", "2026-01-01T11:00:00", "reserve", order="o1", sku="bolt", qty=2),
        ]
        self.assertEqual(apply_events(inv, lines), 2)
        self.assertEqual(inv.stock("bolt"), 5)
        self.assertEqual(inv.available("bolt"), 3)

    def test_ties_keep_file_order_and_fulfil(self):
        inv = Inventory()
        lines = [
            ev("a", "2026-01-01T10:00:00", "add", sku="nut", qty=4),
            ev("b", "2026-01-01T10:00:00", "reserve", order="o1", sku="nut", qty=4),
            ev("c", "2026-01-01T09:00:00", "add", sku="bolt", qty=1),
            ev("d", "2026-01-01T12:00:00", "fulfil", order="o1"),
        ]
        self.assertEqual(apply_events(inv, lines), 4)
        self.assertEqual(inv.stock("nut"), 0)
        self.assertEqual(inv.stock("bolt"), 1)

    def test_sample_log(self):
        inv = Inventory()
        with open("data/events.jsonl") as f:
            n = apply_events(inv, f.readlines())
        self.assertEqual(n, 6)
        self.assertEqual(inv.stock("bolt"), 12)
        self.assertEqual(inv.available("bolt"), 7)
        self.assertEqual(inv.stock("nut"), 3)


if __name__ == "__main__":
    unittest.main()
