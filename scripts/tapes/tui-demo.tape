# VHS Tape: TUI Dashboard Demo
# Generates a GIF showing the terminal UI dashboard
#
# To run: vhs scripts/tapes/tui-demo.tape

Output assets/screenshots/tui-demo.gif

Set Shell bash
Set FontSize 16
Set Width 1200
Set Height 800
Set Theme "Catppuccin Mocha"
Set Padding 24
Set TypingSpeed 50ms

# Launch TUI
Type "aranet tui"
Enter
Sleep 2s

# Navigate around
Sleep 1s
Type "s"  # Scan for devices
Sleep 3s

# Select first device
Type "j"
Sleep 500ms
Enter
Sleep 2s

# Switch to history tab
Type@100ms "Tab"
Sleep 1s

# Switch to settings tab
Type@100ms "Tab"
Sleep 1s

# Back to dashboard
Type@100ms "Tab"
Sleep 1s

# Show help
Type "?"
Sleep 2s

# Dismiss help and quit
Escape
Sleep 500ms
Type "q"
Sleep 500ms

