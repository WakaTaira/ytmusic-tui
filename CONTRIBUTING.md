# Contributing to ytmusic-tui

Thanks for your interest in contributing! Here's how to get started.

## Development Setup

```bash
git clone https://github.com/WakaTaira/ytmusic-tui.git
cd ytmusic-tui
python -m venv .venv
source .venv/bin/activate
pip install -e ".[dev]"
```

### System Dependencies

- **mpv** — audio playback backend
- **yt-dlp** — YouTube stream resolution

On Arch Linux:

```bash
sudo pacman -S mpv yt-dlp python
```

## Code Style

- Format with **ruff format**, lint with **ruff check**
- Type hints are required on all public functions
- Comments and docstrings in English

```bash
ruff format src/ tests/
ruff check src/ tests/
mypy src/ytmusic_tui/
```

## Testing

```bash
# Unit tests only
pytest tests/ -m "not integration"

# All tests (requires browser credentials + network)
pytest tests/
```

Integration tests that hit the YouTube Music API are marked with `@pytest.mark.integration`. These require valid browser credentials at `~/.config/ytmusic-tui/browser.json` (set up via `ytmusic-tui auth`) and network access.

## Pull Request Process

1. Fork the repo and create a feature branch
2. Write tests for new functionality
3. Ensure all checks pass (`ruff`, `mypy`, `pytest`)
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
- Your Python version (`python --version`)
- Your OS and terminal emulator
- Steps to reproduce
- Expected vs actual behavior
