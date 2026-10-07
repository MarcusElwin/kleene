# Installation

Prebuilt binaries cover macOS (Apple silicon and Intel) and Linux (x86_64 and aarch64) and are attached to every tagged release. Pick one of the three.

## curl

```bash
curl -fsSL https://raw.githubusercontent.com/MarcusElwin/kleene/main/install.sh | sh
```

The script detects your OS and architecture, downloads the latest release tarball and the release's `SHA256SUMS`, verifies the checksum and installs to `~/.local/bin` (or `/usr/local/bin` when run as root). It tells you if the destination is not on your `PATH`.

| Variable | Effect | Default |
|---|---|---|
| `KLEENE_VERSION` | install a specific tag, e.g. `v0.1.0` | latest release |
| `KLEENE_INSTALL` | destination directory | `~/.local/bin` |
| `KLEENE_REPO` | `owner/repo` to fetch from | `MarcusElwin/kleene` |
| `GITHUB_TOKEN` (or `GH_TOKEN`) | raises the API rate limit | unset |

[`install.sh`](https://github.com/MarcusElwin/kleene/blob/main/install.sh) is a hundred lines of POSIX `sh` and needs only `curl` and `tar`; read it first if that is your habit.

## Homebrew

```bash
brew tap MarcusElwin/kleene https://github.com/MarcusElwin/kleene
brew trust MarcusElwin/kleene   # Homebrew 7 asks once for third-party taps
brew install kleene
```

The repository is its own tap: `brew tap` with the URL clones it and finds [`Formula/kleene.rb`](https://github.com/MarcusElwin/kleene/blob/main/Formula/kleene.rb), which pins the release tarballs by SHA-256. `brew install --HEAD kleene` builds `main` from source instead (about ten minutes, DuckDB included).

## From source

```bash
cargo install --git https://github.com/MarcusElwin/kleene kleene
```

Needs Rust 1.88 or newer (the repository pins a toolchain in `rust-toolchain.toml`). DuckDB compiles from source the first time, about ten minutes on four cores; after that, rebuilds are quick as long as `target/` is kept.

## Check it

The engine works without a model:

```bash
kleene --version
kleene repl -c "SELECT 1 + 1 AS two"    # prints a one-row table: two = 2
```

Next: [[Quickstart]].
