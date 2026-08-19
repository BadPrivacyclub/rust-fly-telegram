#!/usr/bin/env bash
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

ok()   { echo -e "${GREEN}[OK]${NC}    $*"; }
info() { echo -e "${BLUE}[INFO]${NC}  $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC}  $*"; }
fail() { echo -e "${RED}[ERROR]${NC} $*"; exit 1; }

echo "========================================"
echo "  fly-telegram | Linux/macOS build"
echo "========================================"
echo

if ! command -v cargo &>/dev/null; then
    fail "Rust is not installed.\n\n  Install it with:\n    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh\n\n  Then restart your shell and run this script again."
fi

ok "$(cargo --version)"

RUST_VERSION=$(rustc --version | grep -oE '[0-9]+\.[0-9]+' | head -1)
RUST_MAJOR=$(echo "$RUST_VERSION" | cut -d. -f1)
RUST_MINOR=$(echo "$RUST_VERSION" | cut -d. -f2)
if [ "$RUST_MAJOR" -lt 1 ] || { [ "$RUST_MAJOR" -eq 1 ] && [ "$RUST_MINOR" -lt 75 ]; }; then
    warn "Rust $RUST_VERSION detected. Version 1.75+ is recommended."
    warn "Upgrade with: rustup update stable"
fi

CC_FOUND=false
for compiler in gcc clang cc; do
    if command -v "$compiler" &>/dev/null; then
        ok "C compiler: $($compiler --version 2>&1 | head -1)"
        CC_FOUND=true
        break
    fi
done

if [ "$CC_FOUND" = false ]; then
    echo
    warn "No C compiler found. Lua 5.4 requires one to compile."
    echo
    echo "  Ubuntu / Debian:"
    echo "    sudo apt install build-essential"
    echo
    echo "  Fedora / RHEL:"
    echo "    sudo dnf install gcc"
    echo
    echo "  Arch Linux:"
    echo "    sudo pacman -S base-devel"
    echo
    echo "  macOS:"
    echo "    xcode-select --install"
    echo
    read -rp "Continue anyway? (y/N): " CONTINUE
    if [[ "${CONTINUE,,}" != "y" ]]; then
        exit 1
    fi
fi

if command -v git &>/dev/null; then
    ok "$(git --version)"
else
    info "Git not found, skipping version info"
fi

echo
echo "Building in release mode..."
echo

cargo build --release

echo
echo "========================================"
echo -e "  ${GREEN}Build successful!${NC}"
echo "========================================"
echo
echo "Binary: ./target/release/fly-telegram"
echo
echo "To run:"
echo "  export TELOXIDE_TOKEN=\"your_bot_token_here\""
echo "  ./target/release/fly-telegram"
echo
