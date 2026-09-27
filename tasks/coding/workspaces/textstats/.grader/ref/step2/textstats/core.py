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


def load_stopwords(path):
    """A set of lower-cased words from a file, one per line; blank lines and
    lines starting with '#' are ignored."""
    words = set()
    with open(path, encoding="utf-8") as f:
        for line in f:
            s = line.strip()
            if s and not s.startswith("#"):
                words.add(s.lower())
    return words


def top_words(text, n, stopwords=None):
    """The n most frequent tokens as (word, count), most frequent first,
    ties alphabetical; stopwords excluded."""
    stop = stopwords or set()
    counts = {w: c for w, c in word_counts(text).items() if w not in stop}
    ranked = sorted(counts.items(), key=lambda wc: (-wc[1], wc[0]))
    return ranked[: max(n, 0)]


def ngrams(tokens, n):
    """Consecutive n-tuples of tokens; n < 1 is a ValueError, n longer than
    the list gives no n-grams."""
    if n < 1:
        raise ValueError("n must be at least 1")
    return [tuple(tokens[i : i + n]) for i in range(len(tokens) - n + 1)]
