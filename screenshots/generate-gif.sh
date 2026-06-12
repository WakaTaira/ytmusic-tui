#!/usr/bin/env bash
# Generate the interactive demo GIF for the README.
#
# This script is separate from generate.sh (which produces static PNG screenshots)
# because:
#  - The demo GIF uses YTMUSIC_TUI_DEMO_INTERACTIVE=1, a different code path.
#  - vhs renders .gif output differently from .png (ffmpeg-driven, larger, slower).
#  - generate.sh processes all *.tape in screenshots/tapes/ and expects PNG output;
#    mixing a GIF tape there would require forking the output-existence check.
#
# Requirements:
#   vhs  — https://github.com/charmbracelet/vhs
#          Arch: sudo pacman -S vhs ttyd
#
# Usage:
#   ./screenshots/generate-gif.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

# Guard: vhs must be on PATH.
if ! command -v vhs &>/dev/null; then
    echo "error: 'vhs' not found on PATH."
    echo "  Install on Arch Linux: sudo pacman -S vhs ttyd"
    echo "  More info: https://github.com/charmbracelet/vhs"
    exit 1
fi

# Build the release binary so the tape can reference ./target/release/ytmusic-tui.
echo "==> Building release binary..."
cargo build --release

# Set up a throwaway HOME so the demo session never reads the real user's
# config or browser.json.  The interactive demo skips auth/mpv entirely, but
# the config loader runs before the demo branch and will try to read config.toml
# from the XDG config dir.
export HOME="${SCRIPT_DIR}/.fakehome"
export XDG_CONFIG_HOME="${HOME}/.config"
mkdir -p "${XDG_CONFIG_HOME}"

# Output directory.
mkdir -p "${SCRIPT_DIR}/out"

TAPE="${SCRIPT_DIR}/tapes-gif/demo.tape"
OUT="${SCRIPT_DIR}/out/demo.gif"

echo "==> Rendering demo GIF..."
vhs "${TAPE}"

# vhs occasionally exits 0 without writing the output; retry once.
if [[ ! -f "${OUT}" ]]; then
    echo "    output missing, retrying..."
    vhs "${TAPE}"
fi

if [[ ! -f "${OUT}" ]]; then
    echo "error: ${OUT} was not produced after retry" >&2
    exit 1
fi

# Report file size and frame count (if gifsicle is available).
SIZE=$(du -sh "${OUT}" | cut -f1)
echo ""
echo "Done. GIF written to ${OUT} (${SIZE})"

if command -v gifsicle &>/dev/null; then
    FRAMES=$(gifsicle --info "${OUT}" 2>/dev/null | grep -oP '\d+ images' | head -1 || echo "unknown frames")
    echo "      ${FRAMES}"
fi

# Warn if the file exceeds 3 MB — the README embedding threshold.
SIZE_BYTES=$(stat -c %s "${OUT}" 2>/dev/null || stat -f %z "${OUT}" 2>/dev/null)
if [[ "${SIZE_BYTES}" -gt 3145728 ]]; then
    echo ""
    echo "WARNING: ${OUT} is larger than 3 MB (${SIZE})."
    echo "  Consider reducing Width/Height in the tape or shortening the scenario."
fi
