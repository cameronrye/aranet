# Homebrew formula for Aranet CLI
# This file serves as a template for the cameronrye/homebrew-aranet tap.
#
# To set up the tap repository:
#   1. Create repo: github.com/cameronrye/homebrew-aranet
#   2. Copy this file to Formula/aranet.rb in that repo
#   3. Update SHA256 checksums after each release
#
# Users will install with:
#   brew tap cameronrye/aranet
#   brew install aranet
#
# Or directly:
#   brew install cameronrye/aranet/aranet

class Aranet < Formula
  desc "CLI for Aranet environmental sensors (CO2, radon, temperature, humidity)"
  homepage "https://github.com/cameronrye/aranet"
  version "0.1.8"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/cameronrye/aranet/releases/download/v#{version}/aranet-aarch64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_MACOS_ARM64"
    end
    on_intel do
      url "https://github.com/cameronrye/aranet/releases/download/v#{version}/aranet-x86_64-apple-darwin.tar.gz"
      sha256 "PLACEHOLDER_SHA256_MACOS_X64"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/cameronrye/aranet/releases/download/v#{version}/aranet-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_LINUX_ARM64"
    end
    on_intel do
      url "https://github.com/cameronrye/aranet/releases/download/v#{version}/aranet-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "PLACEHOLDER_SHA256_LINUX_X64"
    end
  end

  # Linux requires D-Bus for Bluetooth
  depends_on "dbus" => :build if OS.linux?

  def install
    bin.install "aranet"
    
    # Install shell completions if present in the archive
    if File.exist?("completions/aranet.bash")
      bash_completion.install "completions/aranet.bash" => "aranet"
    end
    if File.exist?("completions/aranet.zsh")
      zsh_completion.install "completions/aranet.zsh" => "_aranet"
    end
    if File.exist?("completions/aranet.fish")
      fish_completion.install "completions/aranet.fish"
    end
    
    # Generate completions if not present
    unless File.exist?("completions")
      generate_completions_from_executable(bin/"aranet", "completions")
    end
  end

  def caveats
    <<~EOS
      Aranet CLI requires Bluetooth access.

      On macOS:
        Grant Bluetooth permission when prompted, or add Terminal/iTerm
        to System Preferences > Privacy & Security > Bluetooth.

      On Linux:
        Ensure your user is in the 'bluetooth' group:
          sudo usermod -aG bluetooth $USER
        
        You may need to log out and back in for changes to take effect.

      Quick start:
        aranet scan              # Find nearby devices
        aranet read              # Read current values
        aranet watch             # Continuous monitoring
        aranet tui               # Interactive dashboard
    EOS
  end

  test do
    # Test that the binary runs and shows help
    assert_match "CLI for Aranet environmental sensors", shell_output("#{bin}/aranet --help")
    
    # Test version output
    assert_match version.to_s, shell_output("#{bin}/aranet --version")
    
    # Test that config command works (doesn't require Bluetooth)
    system "#{bin}/aranet", "config", "path"
  end
end

