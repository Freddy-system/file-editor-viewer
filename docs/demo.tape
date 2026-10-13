# Records docs/demo.gif with VHS (https://github.com/charmbracelet/vhs).
# Run from the repository root with mdview on PATH:  vhs docs/demo.tape
# VHS draws in a plain terminal without image support, so this shows the
# text mode every terminal gets.
# VHS does not keep a steady pace between runs; the published GIF was retimed
# to 28 seconds with ffmpeg (setpts) afterwards.

Output docs/demo.gif

Set Shell bash
Set FontSize 15
Set Width 1180
Set Height 760
Set Padding 14
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms

Env MDVIEW_GRAPHICS "text"

Type "mdview examples/demo.md"
Sleep 600ms
Enter
Sleep 2.5s

# Scroll through text, lists and the table.
Down@70ms 14
Sleep 1.8s

# Jump to the diagrams through the table of contents.
Type "t"
Sleep 1.4s
Down@220ms 4
Sleep 700ms
Enter
Sleep 3s
Down@90ms 14
Sleep 2.5s

# Search.
Type "/"
Sleep 400ms
Type "greeting"
Sleep 500ms
Enter
Sleep 2s

# Copy the code block on screen.
Type "y"
Sleep 2s

# Follow a link to another document and come back.
Type "g"
Sleep 600ms
Tab
Sleep 900ms
Enter
Sleep 2.2s
Backspace
Sleep 1.2s

# Help.
Type "?"
Sleep 3.5s
Escape
Sleep 500ms
Type "q"
Sleep 400ms
