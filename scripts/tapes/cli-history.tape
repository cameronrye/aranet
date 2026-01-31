# VHS Tape: CLI History Demo
# Generates a GIF showing the `aranet history` command
#
# To run: vhs scripts/tapes/cli-history.tape

Output assets/screenshots/cli-history.gif

Set Shell bash
Set FontSize 16
Set Width 1200
Set Height 800
Set Theme "Catppuccin Mocha"
Set Padding 24
Set TypingSpeed 50ms

# Download measurement history from Aranet4
Type "aranet history --device office --count 10"
Enter
Sleep 6s

# Show CSV export from radon monitor
Sleep 1s

Type "aranet history --device radon --count 5 --format csv"
Enter
Sleep 5s

Sleep 2s

