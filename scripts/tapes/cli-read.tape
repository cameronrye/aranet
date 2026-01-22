# VHS Tape: CLI Read Demo
# Generates a GIF showing the `aranet read` command
#
# To run: vhs scripts/tapes/cli-read.tape

Output assets/screenshots/cli-read.gif

Set Shell bash
Set FontSize 16
Set Width 800
Set Height 500
Set Theme "Catppuccin Mocha"
Set Padding 20
Set TypingSpeed 50ms

# Read current sensor values
Type "aranet read --device living-room"
Enter
Sleep 3s

# Show JSON format
Sleep 1s

Type "aranet read --device living-room --json"
Enter
Sleep 3s

Sleep 2s

