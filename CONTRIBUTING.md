# Contributing to ytmusic-tui

Thanks for your interest in contributing! Here's how to get started.

## Development Setup

```bash
git clone https://github.com/WakaTaira/ytmusic-tui.git
cd ytmusic-tui
cargo build
```

A stable Rust toolchain is pinned via `rust-toolchain.toml`; `rustup` will pick
it up automatically.

### System Dependencies

- **libmpv** — audio playback backend (the linker needs the dev package's
  unversioned `libmpv.so`)
- **mpv** / **yt-dlp** — runtime: mpv's ytdl-hook uses yt-dlp to resolve
  YouTube stream URLs

On Arch Linux:

```bash
sudo pacman -S mpv yt-dlp
```

On Debian/Ubuntu:

```bash
sudo apt install libmpv-dev mpv yt-dlp
```

## Code Style

- Format with **rustfmt** (`cargo fmt`)
- Lint with **clippy**, warnings treated as errors (`cargo clippy --all-targets -- -D warnings`)
- Comments and doc comments in English

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

## Testing

```bash
# Unit + integration tests (no network required)
cargo test

# Live tests that hit the real YouTube Music API
cargo test -p ytmusic-api -- --ignored
```

The `--ignored` tests hit the YouTube Music API. These require valid browser
credentials at `~/.config/ytmusic-tui/browser.json` (see the README for setup)
and network access.

## Pull Request Process

1. Fork the repo and create a feature branch
2. Write tests for new functionality
3. Ensure all checks pass (`cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`)
4. Use [Conventional Commits](https://www.conventionalcommits.org/) for commit messages
5. Open a PR with a clear description of the change

## Commit Messages

```
feat: add shuffle toggle to queue
fix: surface expired cookies in the status bar
refactor: extract mpv IPC into separate module
```

## Reporting Issues

Please include:
- Your `cargo --version` / `rustc --version`
- Your OS and terminal emulator
- Steps to reproduce
- Expected vs actual behavior
