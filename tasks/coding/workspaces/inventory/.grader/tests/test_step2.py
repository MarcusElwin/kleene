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
        self.assertEqual(inv.reserved("bolt"), 4)

    def test_reserve_beyond_available_raises(self):
        inv = Inventory()
        inv.add("bolt", 5)
        inv.reserve("o1", "bolt", 4)
        with self.assertRaises(InsufficientStock) as cm:
            inv.reserve("o2", "bolt", 2)
        self.assertEqual(cm.exception.available, 1)

    def test_remove_respects_reservations(self):
        inv = Inventory()
        inv.add("bolt", 5)
        inv.reserve("o1", "bolt", 4)
        with self.assertRaises(InsufficientStock):
            inv.remove("bolt", 2)
        inv.remove("bolt", 1)
        self.assertEqual(inv.stock("bolt"), 4)

    def test_reserving_again_replaces_the_hold(self):
        inv = Inventory()
        inv.add("bolt", 10)
        inv.reserve("o1", "bolt", 4)
        inv.reserve("o1", "bolt", 7)
        self.assertEqual(inv.available("bolt"), 3)
        inv.reserve("o1", "bolt", 10)
        self.assertEqual(inv.available("bolt"), 0)

    def test_release_and_fulfil(self):
        inv = Inventory()
        inv.add("bolt", 10)
        inv.add("nut", 2)
        inv.reserve("o1", "bolt", 4)
        inv.release("o1")
        inv.release("never")
        self.assertEqual(inv.available("bolt"), 10)
        inv.reserve("o2", "bolt", 3)
        inv.reserve("o2", "nut", 2)
        inv.fulfil("o2")
        self.assertEqual(inv.stock("bolt"), 7)
        self.assertEqual(inv.available("bolt"), 7)
        self.assertEqual(inv.stock("nut"), 0)
        self.assertEqual(inv.skus(), ["bolt"])
        with self.assertRaises(KeyError):
            inv.fulfil("o2")


if __name__ == "__main__":
    unittest.main()
