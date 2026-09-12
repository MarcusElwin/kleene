# Homebrew formula for callgebra. Homebrew formulas live in a tap, so this
# file is a template: copy it to a `homebrew-callgebra` repository as
# `Formula/callgebra.rb`, then users run
#
#   brew install MarcusElwin/callgebra/callgebra
#
# The release workflow (ci/release.yml) prints the version and the four
# sha256 values to paste below after every tagged release.
class Callgebra < Formula
  desc "Relational algebra for recursive model calls: CallSQL engine, planner, harness and TUI"
  homepage "https://github.com/MarcusElwin/callgebra"
  version "0.0.1"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/MarcusElwin/callgebra/releases/download/v#{version}/callgebra-#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256_FROM_RELEASE"
    end
    on_intel do
      url "https://github.com/MarcusElwin/callgebra/releases/download/v#{version}/callgebra-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_SHA256_FROM_RELEASE"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/MarcusElwin/callgebra/releases/download/v#{version}/callgebra-#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256_FROM_RELEASE"
    end
    on_intel do
      url "https://github.com/MarcusElwin/callgebra/releases/download/v#{version}/callgebra-#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_SHA256_FROM_RELEASE"
    end
  end

  def install
    bin.install "callgebra"
    doc.install "DIALECT.md" if File.exist?("DIALECT.md")
  end

  test do
    assert_match "callgebra", shell_output("#{bin}/callgebra --version")
    assert_match "2", shell_output("#{bin}/callgebra repl -c 'SELECT 1 + 1 AS two'")
  end
end
