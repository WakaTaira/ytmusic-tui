# ytmusic-api

Rust client for YouTube Music's internal InnerTube API.

This crate handles browser-cookie authentication, SAPISIDHASH signing, and
provides typed endpoint functions for search, library, playlist, album, artist,
lyrics, history, radio, and mutation operations (like/unlike, playlist
create/add/remove).

**Note:** This uses YouTube's unofficial InnerTube API. It is not sanctioned by
Google and may break at any time.

## Part of ytmusic-tui

This crate is the API layer for [ytmusic-tui](https://github.com/WakaTaira/ytmusic-tui),
a terminal music player for YouTube Music. It can also be used as a standalone
library for building other YouTube Music clients.

## License

MIT
