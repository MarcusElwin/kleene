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
  version "0.4.0"
  license "MIT"

  head do
    url "https://github.com/MarcusElwin/kleene.git", branch: "main"
    depends_on "rust" => :build
  end

  on_macos do
    on_arm do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "5651b66e2f57032d0c8cb97d980c891d99796ef250d1d7ebc864fba0d27ba696"
    end
    on_intel do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "03a296eff75265ec4a865084137475ea1aad886f219c944ebee3bbef2657f3e1"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "be5f2f254283b6d06b2dd73b56681d337332c4fea880856a70daf7b779b93543"
    end
    on_intel do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "468a87a9f19aabe2c2bffa9c63a7facc2b09d2b7b0ac4763bbb1169f26f8c37f"
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
