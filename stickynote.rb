class Stickynote < Formula
  desc "Terminal-based sticky notes board with markdown, tags, and mouse support"
  homepage "https://github.com/Narqulie/stickynote"
  url "https://github.com/Narqulie/stickynote/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "69b70d3d8c68cb03a400a649fe0088d5681de4a6999e88f5ba30fd51e1a7a83e"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "stickynote #{version}", shell_output("#{bin}/stickynote --help")
  end
end
