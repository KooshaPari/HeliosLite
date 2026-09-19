class Forge < Formula
  desc "Fastest AI coding agent — 40+ models, local-first, zero data exfiltration"
  homepage "https://github.com/KooshaPari/forgecode"
  version "2.13.21-h.0.2.7"
  license "MIT"

  on_macos do
    if Hardware::CPU.intel?
      url "https://github.com/KooshaPari/forgecode/releases/download/v#{version}/forge-x86_64-apple-darwin"
      sha256 "318fb6eff8eae19402a46df92270cffe5f55163d9d0c06dfece5966acb512b3f"
    else
      url "https://github.com/KooshaPari/forgecode/releases/download/v#{version}/forge-aarch64-apple-darwin"
      sha256 "4b26bfe8dfcadd371b8cb0d0d6709e67d5da7d048dffb7e0b2827eff1a954d10"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/KooshaPari/forgecode/releases/download/v#{version}/forge-x86_64-unknown-linux-gnu"
      sha256 "74e2209277cc703e4c1bcd5b07a2d1d0a2b0140a99fcd56dada7649335a33c6c"
    else
      url "https://github.com/KooshaPari/forgecode/releases/download/v#{version}/forge-aarch64-unknown-linux-gnu"
      sha256 "8c9c08a87b5155a03eb9176db2646ecc4708b0805e9c532e0f8ef72044b35a5b"
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
