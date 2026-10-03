import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

from scheduler.intervals import parse_time, parse_interval, merge


class ParseTests(unittest.TestCase):
    def test_parse_time(self):
        self.assertEqual(parse_time("09:30"), 570)
        self.assertEqual(parse_time("00:00"), 0)

    def test_parse_interval(self):
        self.assertEqual(parse_interval("09:00-10:30"), (540, 630))

    def test_end_must_follow_start(self):
        with self.assertRaises(ValueError):
            parse_interval("10:00-09:00")


class MergeTests(unittest.TestCase):
    def test_merge_overlapping_and_touching(self):
        self.assertEqual(merge([(540, 600), (570, 630), (630, 660), (720, 780)]), [(540, 660), (720, 780)])

    def test_merge_sorts(self):
        self.assertEqual(merge([(720, 780), (540, 600)]), [(540, 600), (720, 780)])


if __name__ == "__main__":
    unittest.main()
