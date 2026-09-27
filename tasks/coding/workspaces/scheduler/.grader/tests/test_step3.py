import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

from scheduler.overlap import overlaps, total_busy, parse_span


class OverlapTests(unittest.TestCase):
    def test_overlapping(self):
        self.assertTrue(overlaps((540, 600), (570, 630)))
        self.assertTrue(overlaps((540, 700), (560, 600)))

    def test_touching_does_not_overlap(self):
        self.assertFalse(overlaps((540, 600), (600, 660)))
        self.assertFalse(overlaps((600, 660), (540, 600)))

    def test_disjoint(self):
        self.assertFalse(overlaps((540, 600), (610, 660)))


class TotalBusyTests(unittest.TestCase):
    def test_counts_each_minute_once(self):
        self.assertEqual(total_busy([(540, 600), (570, 630)]), 90)
        self.assertEqual(total_busy([(540, 600), (600, 660)]), 120)
        self.assertEqual(total_busy([]), 0)

    def test_overnight_span(self):
        self.assertEqual(parse_span("22:00-02:00"), (1320, 1560))
        self.assertEqual(parse_span("09:00-10:00"), (540, 600))
        self.assertEqual(parse_span("10:00-10:00"), (600, 2040))
        self.assertEqual(total_busy([parse_span("22:00-02:00"), parse_span("23:00-24:00")]), 240)
        with self.assertRaises(ValueError):
            parse_span("22:00")


if __name__ == "__main__":
    unittest.main()
