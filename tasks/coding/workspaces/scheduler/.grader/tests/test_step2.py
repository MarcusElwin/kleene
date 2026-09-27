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

    def test_unsorted_overlapping_busy(self):
        busy = [(660, 720), (540, 600), (570, 640)]
        self.assertEqual(free_slots(busy, 540, 780, 20), [(640, 660), (720, 780)])

    def test_busy_outside_the_day_is_clipped(self):
        busy = [(0, 560), (700, 1440)]
        self.assertEqual(free_slots(busy, 540, 780, 60), [(560, 700)])
        self.assertEqual(free_slots([(0, 1440)], 540, 780, 1), [])

    def test_free_day_and_empty_day(self):
        self.assertEqual(free_slots([], 540, 780, 60), [(540, 780)])
        self.assertEqual(free_slots([], 540, 540, 1), [])

    def test_format(self):
        self.assertEqual(format_interval((540, 630)), "09:00-10:30")
        self.assertEqual(format_interval((0, 1440)), "00:00-24:00")


if __name__ == "__main__":
    unittest.main()
