# VHS Tape: CLI Read Demo
# Generates a GIF showing the `aranet read` command
#
# To run: vhs scripts/tapes/cli-read.tape

Output assets/screenshots/cli-read.gif

Set Shell bash
Set FontSize 16
Set Width 1200
Set Height 800
Set Theme "Catppuccin Mocha"
Set Padding 24
Set TypingSpeed 50ms

# Read from Aranet4 (CO2 monitor)
Type "aranet read --device office"
Enter
Sleep 4s

# Read from AranetRn+ (Radon monitor)
Sleep 1s

Type "aranet read --device radon"
Enter
Sleep 4s

Sleep 2s

