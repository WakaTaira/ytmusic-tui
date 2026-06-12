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

# Go to search page (/ key — ToggleFilter would open filter bar, use g+s to reach search)
Sleep 300ms
Type "/"
Sleep 300ms

# Wait for search view with input focused, type a query
Sleep 500ms
Type "neon cascade"
Sleep 400ms
Enter
Sleep 1200ms

# Navigate to first search result
Sleep 400ms
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

# Back to home
Sleep 400ms
Type "g"
Sleep 600ms

# Open theme popup
Sleep 400ms
Type "T"
Sleep 600ms

# Move down in theme popup to next theme
Type "j"
Sleep 400ms
Type "j"
Sleep 400ms

# Apply selected theme
Enter
Sleep 1500ms

Screenshot screenshots/out/demo-mid.png

# Final pause before quit
Sleep 1s
Type "Q"
Sleep 500ms
