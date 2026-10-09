#!/bin/sh
# Point Formula/kleene.rb at a release: set its version and the sha256 of
# each target's tarball from the release's SHA256SUMS.
#
#   scripts/update-formula.sh 0.2.0 dist/SHA256SUMS
#
# release.yml runs this after it uploads the assets and commits the result
# to main, so the tap serves the new release without a hand-made PR. It is
# also what to run by hand for a release assembled outside the workflow.
# Every target in the formula must have a line in SHA256SUMS; the script
# fails rather than leave a formula that mixes versions.
set -eu

version="${1:?usage: update-formula.sh <version> <SHA256SUMS>}"
sums="${2:?usage: update-formula.sh <version> <SHA256SUMS>}"
formula="$(dirname "$0")/../Formula/kleene.rb"

for target in aarch64-apple-darwin x86_64-apple-darwin \
              aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
  sha="$(awk -v name="kleene-$version-$target.tar.gz" \
    '{ sub(/^\*/, "", $2) } $2 == name { print $1 }' "$sums")"
  if [ -z "$sha" ]; then
    echo "update-formula: no checksum for $target in $sums" >&2
    exit 1
  fi
  # The sha256 line is the one right after this target's url line.
  awk -v target="$target" -v sha="$sha" '
    hit && /^[[:space:]]*sha256 "/ { sub(/"[0-9a-f]*"/, "\"" sha "\""); hit = 0 }
    { print }
    /^[[:space:]]*url ".*-/ && index($0, "-" target ".tar.gz") { hit = 1 }
  ' "$formula" > "$formula.tmp" && mv "$formula.tmp" "$formula"
done

awk -v version="$version" '
  /^[[:space:]]*version "/ { sub(/"[^"]*"/, "\"" version "\"") }
  { print }
' "$formula" > "$formula.tmp" && mv "$formula.tmp" "$formula"

echo "Formula/kleene.rb: version $version"
