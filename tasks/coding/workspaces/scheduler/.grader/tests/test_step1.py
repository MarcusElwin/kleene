import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

from scheduler.intervals import parse_time, parse_interval, merge


class ParseTests(unittest.TestCase):
    def test_parse_time(self):
        self.assertEqual(parse_time("09:30"), 570)
        self.assertEqual(parse_time("00:00"), 0)
        self.assertEqual(parse_time("24:00"), 1440)
        self.assertEqual(parse_time("9:05"), 545)

    def test_parse_time_rejects(self):
        for bad in ("24:01", "25:00", "09:60", "0930", "", "9", "09:3"):
            with self.assertRaises(ValueError):
                parse_time(bad)

    def test_parse_interval(self):
        self.assertEqual(parse_interval("09:00-10:30"), (540, 630))
        self.assertEqual(parse_interval("23:00-24:00"), (1380, 1440))

    def test_end_must_follow_start(self):
        with self.assertRaises(ValueError):
            parse_interval("10:00-09:00")
        with self.assertRaises(ValueError):
            parse_interval("10:00-10:00")
        with self.assertRaises(ValueError):
            parse_interval("10:00")


class MergeTests(unittest.TestCase):
    def test_merge_overlapping_and_touching(self):
        self.assertEqual(merge([(540, 600), (570, 630), (630, 660), (720, 780)]), [(540, 660), (720, 780)])

    def test_merge_sorts(self):
        self.assertEqual(merge([(720, 780), (540, 600)]), [(540, 600), (720, 780)])

    def test_merge_contained_and_empty(self):
        self.assertEqual(merge([(540, 700), (560, 600)]), [(540, 700)])
        self.assertEqual(merge([]), [])
        self.assertEqual(merge([(1, 2)]), [(1, 2)])


if __name__ == "__main__":
    unittest.main()
