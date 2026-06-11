Set FontSize 16
Set Width 1200
Set Height 1000
Set Padding 20
Env YTMUSIC_TUI_DEMO "1"
Env YTMUSIC_TUI_DEMO_SCREEN "home"
Env YTMUSIC_TUI_DEMO_THEME "catppuccin"
Type "./target/release/ytmusic-tui"
Enter
Sleep 2s
Screenshot screenshots/out/home-catppuccin.png
Sleep 1s
