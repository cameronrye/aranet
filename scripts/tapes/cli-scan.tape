# VHS Tape: CLI Scan Demo
# Generates a GIF showing the `aranet scan` command
#
# To run: vhs scripts/tapes/cli-scan.tape

Output assets/screenshots/cli-scan.gif

Set Shell bash
Set FontSize 16
Set Width 1200
Set Height 800
Set Theme "Catppuccin Mocha"
Set Padding 24
Set TypingSpeed 50ms

# Run scan command
Type "aranet scan --timeout 5"
Enter
Sleep 6s

Sleep 2s

