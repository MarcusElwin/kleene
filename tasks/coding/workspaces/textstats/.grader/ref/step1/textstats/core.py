"""Tokenising and counting."""

import re

_TOKEN = re.compile(r"[^\W_]+(?:'[^\W_]+)*", re.UNICODE)


def tokenize(text):
    """Lower-case tokens: maximal runs of letters or digits, an apostrophe
    allowed inside a token (don't, it's) but never at either end; anything
    else, hyphens and underscores included, separates tokens. Tokens of one
    character are dropped."""
    out = []
    for m in _TOKEN.finditer(text.lower()):
        tok = m.group(0).strip("'")
        if len(tok) >= 2:
            out.append(tok)
    return out


def word_counts(text):
    """Token -> count."""
    counts = {}
    for t in tokenize(text):
        counts[t] = counts.get(t, 0) + 1
    return counts
