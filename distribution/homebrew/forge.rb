class Forge < Formula
  desc "Fastest AI coding agent — 40+ models, local-first, zero data exfiltration"
  homepage "https://github.com/KooshaPari/forgecode"
  version "2.13.21-h.0.2.8"
  license "MIT"

  on_macos do
    if Hardware::CPU.intel?
      url "https://github.com/KooshaPari/forgecode/releases/download/v#{version}/forge-x86_64-apple-darwin"
      sha256 "38c44310b96db7662535a1cca459a4f9ea0854d6b8c48a3e21ecbd76ee23571f"
    else
      url "https://github.com/KooshaPari/forgecode/releases/download/v#{version}/forge-aarch64-apple-darwin"
      sha256 "9814d4c3933681bf04a6abcf6c2f8086c88882f07644eb8c841cd05483c61cec"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/KooshaPari/forgecode/releases/download/v#{version}/forge-x86_64-unknown-linux-gnu"
      sha256 "84c5b8de6c5fa4feb786979f50778ad43a01b291985573d36dfa4ce6aed19a7f"
    else
      url "https://github.com/KooshaPari/forgecode/releases/download/v#{version}/forge-aarch64-unknown-linux-gnu"
      sha256 "eee360f9af23c93a083c464db315c21e9c39759663ca970317ef06f8aadac128"
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
