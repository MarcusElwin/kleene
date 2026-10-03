import os
import sys
import unittest

sys.path.insert(0, os.getcwd())

from textstats.core import top_words, load_stopwords, ngrams


class RankingTests(unittest.TestCase):
    def test_top_words_by_count_then_alpha(self):
        self.assertEqual(top_words("bb aa bb cc aa dd", 3), [("aa", 2), ("bb", 2), ("cc", 1)])

    def test_top_words_n_larger_than_vocabulary(self):
        self.assertEqual(top_words("xx yy", 10), [("xx", 1), ("yy", 1)])
        self.assertEqual(top_words("xx yy", 0), [])

    def test_stopwords_excluded(self):
        self.assertEqual(top_words("the cat and the dog", 2, {"the", "and"}), [("cat", 1), ("dog", 1)])

    def test_load_stopwords(self):
        words = load_stopwords("data/stopwords.txt")
        self.assertEqual(words, {"the", "a", "an", "and", "of", "to", "in", "is", "it"})

    def test_load_stopwords_lowercases_and_strips(self):
        with open("tmp_stop.txt", "w") as f:
            f.write("  The \n\n# c\nAND\n")
        try:
            self.assertEqual(load_stopwords("tmp_stop.txt"), {"the", "and"})
        finally:
            os.remove("tmp_stop.txt")

    def test_ngrams(self):
        self.assertEqual(ngrams(["a", "b", "c"], 2), [("a", "b"), ("b", "c")])
        self.assertEqual(ngrams(["a", "b"], 3), [])
        self.assertEqual(ngrams(["a", "b"], 2), [("a", "b")])
        with self.assertRaises(ValueError):
            ngrams(["a"], 0)


if __name__ == "__main__":
    unittest.main()
