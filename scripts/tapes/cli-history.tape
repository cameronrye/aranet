# VHS Tape: CLI History Demo
# Generates a GIF showing the `aranet history` command
#
# To run: vhs scripts/tapes/cli-history.tape

Output assets/screenshots/cli-history.gif

Set Shell bash
Set FontSize 16
Set Width 900
Set Height 600
Set Theme "Catppuccin Mocha"
Set Padding 20
Set TypingSpeed 50ms

# Download measurement history
Type "aranet history --device living-room --count 10"
Enter
Sleep 4s

# Show CSV export
Sleep 1s

Type "aranet history --device living-room --count 5 --format csv"
Enter
Sleep 3s

Sleep 2s

