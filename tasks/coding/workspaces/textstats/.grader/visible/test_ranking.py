import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

from textstats.core import top_words, load_stopwords, ngrams


class RankingTests(unittest.TestCase):
    def test_top_words_by_count_then_alpha(self):
        self.assertEqual(top_words("bb aa bb cc aa dd", 3), [("aa", 2), ("bb", 2), ("cc", 1)])

    def test_stopwords_excluded(self):
        self.assertEqual(top_words("the cat and the dog", 2, {"the", "and"}), [("cat", 1), ("dog", 1)])

    def test_load_stopwords(self):
        words = load_stopwords("data/stopwords.txt")
        self.assertIn("the", words)
        self.assertNotIn("#", "".join(words))

    def test_ngrams(self):
        self.assertEqual(ngrams(["a", "b", "c"], 2), [("a", "b"), ("b", "c")])


if __name__ == "__main__":
    unittest.main()
