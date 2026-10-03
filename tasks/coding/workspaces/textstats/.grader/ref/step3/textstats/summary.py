"""Whole-file summaries."""

from textstats.core import tokenize, top_words


def summarize(path):
    """lines, words, unique and the top three words of a text file.

    Files may be UTF-8 with a byte-order mark and may use CRLF line ends;
    `lines` counts lines the way a reader would (a final line without a
    trailing newline still counts, and an empty file has none).
    """
    with open(path, encoding="utf-8-sig", newline="") as f:
        text = f.read()
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    lines = len(text.splitlines())
    tokens = tokenize(text)
    return {
        "lines": lines,
        "words": len(tokens),
        "unique": len(set(tokens)),
        "top": top_words(text, 3),
    }
