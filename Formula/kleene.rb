# Homebrew formula for kleene. Homebrew only installs formulae that live in
# a tap, and this repository is its own tap:
#
#   brew tap MarcusElwin/kleene https://github.com/MarcusElwin/kleene
#   brew trust MarcusElwin/kleene   # Homebrew 7 asks once for third-party taps
#   brew install kleene
#   brew install --HEAD kleene   # build main with cargo instead, ~10 min
#
# The stable stanzas download release binaries; the release workflow
# (.github/workflows/release.yml) prints the version and the four sha256
# values to paste below after every release.
class Kleene < Formula
  desc "Relational algebra for recursive model calls: CallSQL engine, planner, harness and TUI"
  homepage "https://kleene.sh"
  version "0.3.1"
  license "MIT"

  head do
    url "https://github.com/MarcusElwin/kleene.git", branch: "main"
    depends_on "rust" => :build
  end

  on_macos do
    on_arm do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "b83c1bd06230be4341b73a0ee2a567317a135886fac378c1395043eb4ae13b01"
    end
    on_intel do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "c1b3f76f4a9e1e029c2a0e3b43bf431dbdfdb897b479319494d1f1bd0cb3bbb3"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "1abdebb5d021d569d1b8dff7f647162cf99eb68c46cb702442b1329dc09c4a9f"
    end
    on_intel do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "c3358ec339dbb6b63691734f4b5a3c81babc7654cb4de2e883d3f51ea2059960"
    end
  end

  def install
    if build.head?
      # DuckDB is compiled from source inside this step.
      system "cargo", "install", *std_cargo_args(path: "crates/kleene")
      doc.install "docs/DIALECT.md", "docs/CLI.md"
    else
      bin.install "kleene"
      doc.install "DIALECT.md" if File.exist?("DIALECT.md")
    end
  end

  test do
    assert_match "kleene", shell_output("#{bin}/kleene --version")
    assert_match "2", shell_output("#{bin}/kleene repl -c 'SELECT 1 + 1 AS two'")
  end
end
