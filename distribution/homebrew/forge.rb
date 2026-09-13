class Forge < Formula
  desc "Fastest AI coding agent — 40+ models, local-first, zero data exfiltration"
  homepage "https://github.com/KooshaPari/forgecode"
  version "2.13.21-h.0.1.8"
  license "MIT"

  on_macos do
    if Hardware::CPU.intel?
      url "https://github.com/KooshaPari/forgecode/releases/download/v#{version}/forge-x86_64-apple-darwin"
      sha256 "MISSING"
    else
      url "https://github.com/KooshaPari/forgecode/releases/download/v#{version}/forge-aarch64-apple-darwin"
      sha256 "MISSING"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/KooshaPari/forgecode/releases/download/v#{version}/forge-x86_64-unknown-linux-gnu"
      sha256 "MISSING"
    else
      url "https://github.com/KooshaPari/forgecode/releases/download/v#{version}/forge-aarch64-unknown-linux-gnu"
      sha256 "MISSING"
    end
  end

  def install
    bin.install Dir["forge*"].first => "forge"
    bin.install Dir["forge_dbd*"].first => "forge_dbd" if Dir["forge_dbd*"].any?
  end

  test do
    assert_match "forge", shell_output("#{bin}/forge --version")
  end
end
