import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

from scheduler.intervals import free_slots, format_interval


class FreeSlotTests(unittest.TestCase):
    def test_gaps_between_busy(self):
        busy = [(540, 600), (660, 720)]
        self.assertEqual(free_slots(busy, 540, 780, 30), [(600, 660), (720, 780)])

    def test_min_length_filters(self):
        busy = [(540, 600), (620, 720)]
        self.assertEqual(free_slots(busy, 540, 780, 30), [(720, 780)])

    def test_format(self):
        self.assertEqual(format_interval((540, 630)), "09:00-10:30")


if __name__ == "__main__":
    unittest.main()
