import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

from inventory.store import Inventory, InsufficientStock


class StockTests(unittest.TestCase):
    def test_add_and_stock(self):
        inv = Inventory()
        inv.add("bolt", 10)
        inv.add("bolt", 5)
        self.assertEqual(inv.stock("bolt"), 15)
        self.assertEqual(inv.stock("nut"), 0)

    def test_remove(self):
        inv = Inventory()
        inv.add("bolt", 10)
        inv.remove("bolt", 4)
        self.assertEqual(inv.stock("bolt"), 6)

    def test_remove_too_many_raises(self):
        inv = Inventory()
        inv.add("bolt", 3)
        with self.assertRaises(InsufficientStock) as cm:
            inv.remove("bolt", 5)
        self.assertEqual(str(cm.exception), "insufficient stock for bolt: requested 5, available 3")

    def test_skus_sorted_with_stock(self):
        inv = Inventory()
        inv.add("washer", 1)
        inv.add("bolt", 2)
        inv.remove("washer", 1)
        self.assertEqual(inv.skus(), ["bolt"])


if __name__ == "__main__":
    unittest.main()
