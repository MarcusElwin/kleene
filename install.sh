#!/bin/sh
# Install the kleene binary from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/MarcusElwin/kleene/main/install.sh | sh
#
# Options (environment variables):
#   KLEENE_VERSION   tag to install, e.g. v0.1.0 (default: latest release)
#   KLEENE_INSTALL   directory to install into (default: ~/.local/bin,
#                       or /usr/local/bin when run as root)
#   KLEENE_REPO      owner/repo (default: MarcusElwin/kleene)
#   GITHUB_TOKEN        (or GH_TOKEN) a token with read access; needed while
#                       the repository is private, and raises the API rate
#                       limit otherwise
set -eu

REPO="${KLEENE_REPO:-MarcusElwin/kleene}"
BIN="kleene"

say() { printf '%s\n' "$*" >&2; }
die() { say "install.sh: $*"; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required"; }
need curl
need tar

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux) os_part="unknown-linux-gnu" ;;
  Darwin) os_part="apple-darwin" ;;
  *) die "unsupported OS: $os (build from source with: cargo install --git https://github.com/$REPO kleene)" ;;
esac
case "$arch" in
  x86_64|amd64) arch_part="x86_64" ;;
  arm64|aarch64) arch_part="aarch64" ;;
  *) die "unsupported architecture: $arch" ;;
esac
target="$arch_part-$os_part"

api="https://api.github.com/repos/$REPO"
token="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
# Every GitHub request goes through here so the token, when given, is sent
# consistently. Output goes to stdout; a failing status returns non-zero.
fetch() {
  if [ -n "$token" ]; then
    curl -fsSL -H "Authorization: Bearer $token" "$@"
  else
    curl -fsSL "$@"
  fi
}

if [ -n "${KLEENE_VERSION:-}" ]; then
  release_url="$api/releases/tags/$KLEENE_VERSION"
else
  release_url="$api/releases/latest"
fi
release="$(fetch -H "Accept: application/vnd.github+json" "$release_url" 2>/dev/null)" || {
  if [ -n "$token" ]; then
    die "no release found at $release_url (has a v* tag been pushed and built yet?)"
  else
    die "no release found at $release_url: either none has been published yet, or \
$REPO is private and needs GITHUB_TOKEN set. To build from source instead: \
cargo install --git https://github.com/$REPO kleene"
  fi
}
tag="$(printf '%s' "$release" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
[ -n "$tag" ] || die "could not read the release tag from $release_url"
version="${tag#v}"
name="$BIN-$version-$target"

# Release assets are downloaded through the API asset URL with the
# octet-stream accept header. That path works for public and private
# repositories alike; the browser download URL does not for private ones.
asset_url() {
  # One key per line, then remember the last asset url and print it when the
  # matching name comes along.
  printf '%s' "$release" | tr ',' '\n' | awk -v want="$1" '
    /"url": *"[^"]*\/releases\/assets\/[0-9]+"/ { sub(/.*"url": *"/, ""); sub(/".*/, ""); url = $0 }
    /"name": *"/ { n = $0; sub(/.*"name": *"/, "", n); sub(/".*/, "", n); if (n == want && url != "") { print url; exit } }
  '
}
tarball_url="$(asset_url "$name.tar.gz")"
sum_url="$(asset_url "$name.tar.gz.sha256")"
[ -n "$tarball_url" ] || die "release $tag has no asset $name.tar.gz (unsupported target, or the release build is still running)"
[ -n "$sum_url" ] || die "release $tag has no checksum for $name.tar.gz"

if [ -n "${KLEENE_INSTALL:-}" ]; then
  dest="$KLEENE_INSTALL"
elif [ "$(id -u)" = "0" ]; then
  dest="/usr/local/bin"
else
  dest="$HOME/.local/bin"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "downloading $name.tar.gz"
fetch -H "Accept: application/octet-stream" -o "$tmp/$name.tar.gz" "$tarball_url"
fetch -H "Accept: application/octet-stream" -o "$tmp/$name.tar.gz.sha256" "$sum_url"

expected="$(awk '{print $1}' "$tmp/$name.tar.gz.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$name.tar.gz" | awk '{print $1}')"
else
  actual="$(shasum -a 256 "$tmp/$name.tar.gz" | awk '{print $1}')"
fi
[ "$expected" = "$actual" ] || die "checksum mismatch for $name.tar.gz"

tar -C "$tmp" -xzf "$tmp/$name.tar.gz"
mkdir -p "$dest"
install -m 755 "$tmp/$name/$BIN" "$dest/$BIN"

say "installed $BIN $version to $dest/$BIN"
case ":$PATH:" in
  *":$dest:"*) ;;
  *) say "note: $dest is not on your PATH; add it with: export PATH=\"$dest:\$PATH\"" ;;
esac
"$dest/$BIN" --version >&2 || true
