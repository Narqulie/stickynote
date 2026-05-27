class Stickynote < Formula
  desc "Terminal-based sticky notes board with markdown, tags, and mouse support"
  homepage "https://github.com/Narqulie/stickynote"
  url "https://github.com/Narqulie/stickynote/archive/refs/tags/v0.2.0.tar.gz"
  sha256 "2fae944f85f168b03379bc724a14063a69052075fb14ae1489f7ee82a8104e16"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "stickynote #{version}", shell_output("#{bin}/stickynote --help")
  end
end
