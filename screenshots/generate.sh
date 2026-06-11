#!/usr/bin/env bash
# Generate all README screenshots via vhs.
#
# Requirements:
#   vhs  — https://github.com/charmbracelet/vhs
#          Arch: sudo pacman -S vhs ttyd
#
# Usage:
#   ./screenshots/generate.sh

set -euo pipefail

# Resolve repo root from the script's own location so the script works
# regardless of which directory it is invoked from.
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

# Build the release binary so the tapes can reference ./target/release/ytmusic-tui.
echo "==> Building release binary..."
cargo build --release

# Set up a throwaway HOME so the demo session never reads the real user's
# config or browser.json.  The demo path skips auth/mpv entirely, but the
# config loader runs before the demo branch and will try to read config.toml
# from the XDG config dir.
export HOME="${SCRIPT_DIR}/.fakehome"
export XDG_CONFIG_HOME="${HOME}/.config"
mkdir -p "${XDG_CONFIG_HOME}"

# Output directory for generated PNG files.
mkdir -p "${SCRIPT_DIR}/out"

# Run every tape in alphabetical order.
TAPES=("${SCRIPT_DIR}/tapes"/*.tape)
TOTAL="${#TAPES[@]}"
COUNT=0

for tape in "${TAPES[@]}"; do
    name="$(basename "${tape}" .tape)"
    out="${SCRIPT_DIR}/out/${name}.png"
    COUNT=$((COUNT + 1))
    echo "==> [${COUNT}/${TOTAL}] Rendering ${name}..."
    vhs "${tape}"
    # vhs occasionally exits 0 without writing the PNG; retry once.
    if [[ ! -f "${out}" ]]; then
        echo "    output missing, retrying ${name}..."
        vhs "${tape}"
    fi
    if [[ ! -f "${out}" ]]; then
        echo "error: ${out} was not produced after retry" >&2
        exit 1
    fi
done

echo ""
echo "Done. ${COUNT} screenshot(s) written to ${SCRIPT_DIR}/out/"
