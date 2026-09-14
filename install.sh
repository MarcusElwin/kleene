#!/bin/sh
# Install the callgebra binary from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/MarcusElwin/callgebra/main/install.sh | sh
#
# Options (environment variables):
#   CALLGEBRA_VERSION   tag to install, e.g. v0.1.0 (default: latest release)
#   CALLGEBRA_INSTALL   directory to install into (default: ~/.local/bin,
#                       or /usr/local/bin when run as root)
#   CALLGEBRA_REPO      owner/repo (default: MarcusElwin/callgebra)
set -eu

REPO="${CALLGEBRA_REPO:-MarcusElwin/callgebra}"
BIN="callgebra"

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
  *) die "unsupported OS: $os (build from source with: cargo install --git https://github.com/$REPO callgebra)" ;;
esac
case "$arch" in
  x86_64|amd64) arch_part="x86_64" ;;
  arm64|aarch64) arch_part="aarch64" ;;
  *) die "unsupported architecture: $arch" ;;
esac
target="$arch_part-$os_part"

if [ -n "${CALLGEBRA_VERSION:-}" ]; then
  tag="$CALLGEBRA_VERSION"
else
  tag="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
  [ -n "$tag" ] || die "could not determine the latest release of $REPO"
fi
version="${tag#v}"
name="$BIN-$version-$target"
base="https://github.com/$REPO/releases/download/$tag"

if [ -n "${CALLGEBRA_INSTALL:-}" ]; then
  dest="$CALLGEBRA_INSTALL"
elif [ "$(id -u)" = "0" ]; then
  dest="/usr/local/bin"
else
  dest="$HOME/.local/bin"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "downloading $name.tar.gz"
curl -fsSL -o "$tmp/$name.tar.gz" "$base/$name.tar.gz"
curl -fsSL -o "$tmp/$name.tar.gz.sha256" "$base/$name.tar.gz.sha256"

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
