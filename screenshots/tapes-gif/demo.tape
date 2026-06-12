Output screenshots/out/demo.gif
Set FontSize 16
Set Width 1200
Set Height 800
Set Padding 16
Env YTMUSIC_TUI_DEMO "1"
Env YTMUSIC_TUI_DEMO_INTERACTIVE "1"
Env YTMUSIC_TUI_DEMO_THEME "synthwave"

Type "./target/release/ytmusic-tui"
Enter
Sleep 2s

# Navigate down in home view — pick a track
Sleep 500ms
Type "j"
Sleep 300ms
Type "j"
Sleep 300ms
Type "j"
Sleep 400ms

# Play selected track (Enter)
Enter
Sleep 800ms

# Go to search page: g then s (two-key sequence; / = ToggleFilter, not search page)
Sleep 300ms
Type "g"
Sleep 200ms
Type "s"
Sleep 600ms

# Search input is now focused (input_mode=true); type query directly
Type "neon cascade"
Sleep 400ms
Enter
Sleep 1200ms

# Leave input mode so j/k navigate the results list
Escape
Sleep 300ms

# Navigate to a search result
Type "j"
Sleep 300ms
Type "j"
Sleep 300ms

# Play the selected track
Enter
Sleep 800ms

# Open queue view
Sleep 400ms
Type "q"
Sleep 800ms

# Pop back to search view, then pop back to home
Escape
Sleep 400ms
Escape
Sleep 500ms

# Open theme popup (T = open_theme_popup; no g prefix needed)
Type "T"
Sleep 600ms

# Themes are sorted alphabetically: catppuccin(0) gruvbox(1) nord(2) synthwave(3)
# Initial theme is synthwave → cursor starts at index 3 (the last entry).
# Move UP once to land on nord (index 2).
Type "k"
Sleep 400ms

# Apply selected theme (nord)
Enter
Sleep 1500ms

Screenshot screenshots/out/demo-mid.png

# Final pause before quit
Sleep 1s
Type "Q"
Sleep 500ms
