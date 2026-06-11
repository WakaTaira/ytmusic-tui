Set FontSize 16
Set Width 1200
Set Height 880
Set Padding 20
Env YTMUSIC_TUI_DEMO "1"
Env YTMUSIC_TUI_DEMO_SCREEN "library"
Type "./target/release/ytmusic-tui"
Enter
Sleep 2s
Screenshot screenshots/out/library.png
Sleep 1s
