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
  version "0.1.0"
  license "MIT"

  head do
    url "https://github.com/MarcusElwin/kleene.git", branch: "main"
    depends_on "rust" => :build
  end

  on_macos do
    on_arm do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "2e005a70e27d1216e969a836a3dc596aaf1c912b27f2afe91a8ab727015f70be"
    end
    on_intel do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "87cdd016ce0c04f4b87cd04f48be462ceba1b11260d4b595f8171f32253d290b"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "f8006717b21b04be6bcb7b402bdff332e66c9fd8d0bff441f3767154efd8ca70"
    end
    on_intel do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "a1fc46f2992b2bb481513bff6db4fbfdc3d37df2de48889d75a5d250fe5942a3"
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
