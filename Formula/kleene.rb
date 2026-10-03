# Homebrew formula for kleene. Homebrew only installs formulae that live in
# a tap, so this file is the template for the `homebrew-kleene` repository
# (as `Formula/kleene.rb`), after which users run
#
#   brew install MarcusElwin/kleene/kleene
#
# Until that tap exists, or to build the current main from source, put it in
# a local tap:
#
#   brew tap-new marcuselwin/kleene
#   cp Formula/kleene.rb "$(brew --repository marcuselwin/kleene)/Formula/"
#   brew install --HEAD marcuselwin/kleene/kleene   # cargo build, ~10 min
#
# The stable stanzas download release binaries; the release workflow
# (ci/release.yml) prints the version and the four sha256 values to paste
# below after every tagged release. `--HEAD` clones the repository and builds
# with cargo, which also works while the repository is private, provided git
# can authenticate (gh auth setup-git, or an SSH remote).
class Kleene < Formula
  desc "Relational algebra for recursive model calls: CallSQL engine, planner, harness and TUI"
  homepage "https://github.com/MarcusElwin/kleene"
  version "0.1.0"
  license "MIT"

  head do
    url "https://github.com/MarcusElwin/kleene.git", branch: "main"
    depends_on "rust" => :build
  end

  on_macos do
    on_arm do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256_FROM_RELEASE"
    end
    on_intel do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256_FROM_RELEASE"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256_FROM_RELEASE"
    end
    on_intel do
      url "https://github.com/MarcusElwin/kleene/releases/download/v#{version}/kleene-#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256_FROM_RELEASE"
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
