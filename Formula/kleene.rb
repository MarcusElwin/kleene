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
  version "0.5.0"
  license "MIT"

  head do
    url "https://github.com/MarcusElwin/kleene.git", branch: "main"
    depends_on "rust" => :build
  end

  on_macos do
    on_arm do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "586af4e726bb0d6dc706b55340344ec392d228f55ea881b1f84ee944fde20c93"
    end
    on_intel do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "2a81188801d6ea9b86aa987bb4eeb0d716272e3ec1d11f1583e92eb753997ed5"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "c1b13ac73d29297c51f9b20b11dddc7b18a1067274d76b0a8b6567565f20737e"
    end
    on_intel do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "240cc19ca38a8ded08ba37b9222252149b8f56e8a69ca938aa99f9b6c152442b"
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
