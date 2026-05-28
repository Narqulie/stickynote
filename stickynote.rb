class Stickynote < Formula
  desc "Terminal-based sticky notes board with markdown, tags, and mouse support"
  homepage "https://github.com/Narqulie/stickynote"
  url "https://github.com/Narqulie/stickynote/archive/refs/tags/v0.3.0.tar.gz"
  sha256 "350e7948f9faf132ed810936b085c6ebf5d8bb7fef2864e1485895d36fd37b5a"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "stickynote #{version}", shell_output("#{bin}/stickynote --help")
  end
end
