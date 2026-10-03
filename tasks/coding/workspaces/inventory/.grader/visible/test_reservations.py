import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

from inventory.store import Inventory, InsufficientStock


class ReservationTests(unittest.TestCase):
    def test_reserve_reduces_available_not_stock(self):
        inv = Inventory()
        inv.add("bolt", 10)
        inv.reserve("o1", "bolt", 4)
        self.assertEqual(inv.stock("bolt"), 10)
        self.assertEqual(inv.available("bolt"), 6)

    def test_reserve_beyond_available_raises(self):
        inv = Inventory()
        inv.add("bolt", 5)
        inv.reserve("o1", "bolt", 4)
        with self.assertRaises(InsufficientStock):
            inv.reserve("o2", "bolt", 2)

    def test_release_and_fulfil(self):
        inv = Inventory()
        inv.add("bolt", 10)
        inv.reserve("o1", "bolt", 4)
        inv.release("o1")
        self.assertEqual(inv.available("bolt"), 10)
        inv.reserve("o2", "bolt", 3)
        inv.fulfil("o2")
        self.assertEqual(inv.stock("bolt"), 7)
        self.assertEqual(inv.available("bolt"), 7)


if __name__ == "__main__":
    unittest.main()
