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
  version "0.3.0"
  license "MIT"

  head do
    url "https://github.com/MarcusElwin/kleene.git", branch: "main"
    depends_on "rust" => :build
  end

  on_macos do
    on_arm do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "8d3b9e27d716ba89c3350bcf31c6f58548a7c0aeb1fa5067123f78272915779a"
    end
    on_intel do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "552c460762d80e36c99676cb98c67c25826d069d9cf8a2f4aced728f94cdd4b6"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "dccc078df7e8765ee3b7d560c0cfef5f8f1802b3dce06fc5a9a30500b7214e6f"
    end
    on_intel do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "1623cda2a291689f73778d301f135650be1d2e39541aa68bc05fd360d87239b3"
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
