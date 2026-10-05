#!/bin/sh
# Publish these pages to the GitHub wiki of MarcusElwin/kleene.
#
# One-time, in the browser: Settings → General → Features → tick "Wikis",
# then open https://github.com/MarcusElwin/kleene/wiki and create the first
# page (any title, any text). GitHub creates the wiki's git repository only
# when the first page is saved; this script replaces that page.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
git clone https://github.com/MarcusElwin/kleene.wiki.git "$tmp/wiki"
cp "$here"/*.md "$tmp/wiki/"
cd "$tmp/wiki"
git add -A
git commit -m "Wiki: Home, Installation, Quickstart, Configuration, CallSQL, CLI, Architecture, Benchmarks, FAQ, Contributing"
git push origin HEAD
echo "published: https://github.com/MarcusElwin/kleene/wiki"
