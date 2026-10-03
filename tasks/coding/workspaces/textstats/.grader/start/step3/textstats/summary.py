"""Whole-file summaries."""

from textstats.core import tokenize, top_words


def summarize(path):
    """lines, words, unique and the top three words of a text file."""
    text = open(path, encoding="latin-1").read()
    lines = len(text.split("\n"))
    tokens = tokenize(text)
    return {
        "lines": lines,
        "words": len(tokens),
        "unique": len(set(tokens)),
        "top": top_words(text, 3),
    }
