import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

from textstats.core import tokenize, word_counts


class TokenizeTests(unittest.TestCase):
    def test_lowercases_and_splits_on_punctuation(self):
        self.assertEqual(tokenize("The quick, brown fox!"), ["the", "quick", "brown", "fox"])

    def test_keeps_inner_apostrophes(self):
        self.assertEqual(tokenize("It doesn't matter"), ["it", "doesn't", "matter"])

    def test_drops_single_characters(self):
        self.assertEqual(tokenize("a b cd"), ["cd"])

    def test_word_counts(self):
        self.assertEqual(word_counts("dog cat dog"), {"dog": 2, "cat": 1})


if __name__ == "__main__":
    unittest.main()
