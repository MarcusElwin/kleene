import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

from textstats.summary import summarize


class SummaryTests(unittest.TestCase):
    def _write(self, name, data):
        with open(name, "wb") as f:
            f.write(data)
        self.addCleanup(os.remove, name)

    def test_plain_file(self):
        self._write("tmp_a.txt", b"dog cat\ndog\n")
        self.assertEqual(summarize("tmp_a.txt"), {"lines": 2, "words": 3, "unique": 2, "top": [("dog", 2), ("cat", 1)]})

    def test_bom_and_crlf(self):
        self._write("tmp_b.txt", "\ufeffcafé dog\r\ndog\r\n".encode("utf-8"))
        s = summarize("tmp_b.txt")
        self.assertEqual(s["lines"], 2)
        self.assertEqual(s["top"], [("dog", 2), ("café", 1)])

    def test_last_line_without_newline_counts(self):
        self._write("tmp_c.txt", b"one two\nthree")
        self.assertEqual(summarize("tmp_c.txt")["lines"], 2)

    def test_empty_file(self):
        self._write("tmp_d.txt", b"")
        self.assertEqual(summarize("tmp_d.txt"), {"lines": 0, "words": 0, "unique": 0, "top": []})

    def test_sample(self):
        s = summarize("data/sample.txt")
        self.assertEqual(s["lines"], 3)
        self.assertEqual(s["top"][0], ("the", 5))


if __name__ == "__main__":
    unittest.main()
