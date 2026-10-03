#!/usr/bin/env bash
# fly-telegram installer for Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/BadPrivacyclub/rust-fly-telegram/main/install.sh | bash
#
# Options (pass after `bash -s --` when piping):
#   --dir <path>      where to put the executable   (default: ~/.local/bin)
#   --data <path>     data directory                (default: ~/fly-telegram)
#   --version <tag>   release tag, e.g. v0.2.0      (default: latest)
#   --source          build from source instead of downloading a release
#   --service         install and start a background service when done
#   --start           start fly-telegram in the foreground when done
set -euo pipefail

REPO="BadPrivacyclub/rust-fly-telegram"
BIN_DIR="${FLY_BIN_DIR:-$HOME/.local/bin}"
DATA_DIR="${FLY_DATA_DIR:-$HOME/fly-telegram}"
VERSION="latest"
FROM_SOURCE=0
SERVICE=0
START=0

bold=$'\033[1m'; green=$'\033[32m'; yellow=$'\033[33m'; red=$'\033[31m'; reset=$'\033[0m'
info() { printf '%s›%s %s\n' "$bold" "$reset" "$*"; }
ok()   { printf '%s✓%s %s\n' "$green" "$reset" "$*"; }
warn() { printf '%s!%s %s\n' "$yellow" "$reset" "$*"; }
die()  { printf '%s✗ %s%s\n' "$red" "$*" "$reset" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --dir) BIN_DIR="$2"; shift 2 ;;
    --data) DATA_DIR="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --source) FROM_SOURCE=1; shift ;;
    --service) SERVICE=1; shift ;;
    --start) START=1; shift ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done

need() { command -v "$1" >/dev/null 2>&1; }

detect_target() {
  local os arch
  os="$(uname -s)"; arch="$(uname -m)"
  case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *) echo ""; return ;;
  esac
  case "$os" in
    Linux) echo "${arch}-unknown-linux-gnu" ;;
    Darwin) echo "${arch}-apple-darwin" ;;
    *) echo "" ;;
  esac
}

download() {
  local url="$1" out="$2"
  if need curl; then curl -fsSL --retry 3 -o "$out" "$url"
  elif need wget; then wget -q -O "$out" "$url"
  else die "curl or wget is required"; fi
}

TMP_DIR=""
cleanup() { [ -n "$TMP_DIR" ] && rm -rf "$TMP_DIR"; return 0; }
trap cleanup EXIT

install_release() {
  local target="$1" tmp base archive
  TMP_DIR="$(mktemp -d)"
  tmp="$TMP_DIR"
  if [ "$VERSION" = "latest" ]; then
    base="https://github.com/$REPO/releases/latest/download"
  else
    base="https://github.com/$REPO/releases/download/$VERSION"
  fi
  archive="fly-telegram-$target.tar.gz"
  info "Downloading $archive ($VERSION)"
  download "$base/$archive" "$tmp/$archive" || return 1
  if download "$base/sha256sums.txt" "$tmp/sha256sums.txt" 2>/dev/null; then
    local expected actual
    expected="$(grep " $archive\$" "$tmp/sha256sums.txt" | awk '{print $1}')"
    if need sha256sum; then actual="$(sha256sum "$tmp/$archive" | awk '{print $1}')"
    else actual="$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')"; fi
    [ -n "$expected" ] && [ "$expected" != "$actual" ] && die "checksum mismatch for $archive"
    ok "Checksum verified"
  fi
  tar -xzf "$tmp/$archive" -C "$tmp"
  mkdir -p "$BIN_DIR"
  install -m 0755 "$tmp/fly-telegram-$target/fly-telegram" "$BIN_DIR/fly-telegram"
  install -m 0755 "$tmp/fly-telegram-$target/music-worker" "$BIN_DIR/fly-telegram-music-worker"
}

install_source() {
  need git || die "git is required to build from source"
  if ! need cargo; then
    warn "Rust is not installed."
    printf 'Install it now with rustup? [Y/n] '
    read -r answer </dev/tty || answer="y"
    case "$answer" in
      n|N) die "Rust is required: https://rustup.rs" ;;
    esac
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  fi
  need cc || need gcc || need clang || die "a C compiler is required (build-essential / Xcode command line tools)"
  local src="$DATA_DIR/src"
  if [ -d "$src/.git" ]; then
    info "Updating $src"
    git -C "$src" pull --ff-only
  else
    info "Cloning into $src"
    git clone --depth 1 "https://github.com/$REPO.git" "$src"
  fi
  info "Building (this takes a few minutes)"
  (cd "$src" && cargo build --release --locked)
  mkdir -p "$BIN_DIR"
  install -m 0755 "$src/target/release/fly-telegram" "$BIN_DIR/fly-telegram"
  install -m 0755 "$src/target/release/music-worker" "$BIN_DIR/fly-telegram-music-worker"
}

main() {
  printf '\n  %s✈ fly-telegram installer%s\n\n' "$bold" "$reset"
  local target
  target="$(detect_target)"

  if [ "$FROM_SOURCE" -eq 0 ] && [ -n "$target" ]; then
    if ! install_release "$target"; then
      warn "No prebuilt release for $target yet; building from source instead."
      install_source
    fi
  else
    [ -z "$target" ] && warn "No prebuilt binary for $(uname -s)/$(uname -m); building from source."
    install_source
  fi
  ok "Installed $BIN_DIR/fly-telegram ($("$BIN_DIR/fly-telegram" --version))"

  mkdir -p "$DATA_DIR"
  "$BIN_DIR/fly-telegram" --data-dir "$DATA_DIR" init >/dev/null
  ok "Data directory: $DATA_DIR"

  case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) warn "$BIN_DIR is not in PATH. Add this to your shell profile:"
       printf '    export PATH="%s:$PATH"\n' "$BIN_DIR" ;;
  esac

  if [ "$SERVICE" -eq 1 ]; then
    if ! "$BIN_DIR/fly-telegram" --data-dir "$DATA_DIR" service install; then
      warn "Could not install the background service; start fly-telegram manually instead."
      SERVICE=0
    fi
  fi

  printf '\n%sNext steps%s\n' "$bold" "$reset"
  if [ "$SERVICE" -eq 1 ]; then
    printf '  Open %shttp://127.0.0.1:8080%s and follow the setup wizard.\n' "$bold" "$reset"
  else
    printf '  1. Run:   %sfly-telegram --data-dir "%s"%s\n' "$bold" "$DATA_DIR" "$reset"
    printf '  2. Open:  %shttp://127.0.0.1:8080%s and follow the setup wizard\n' "$bold" "$reset"
    printf '  3. Later: %sfly-telegram --data-dir "%s" service install%s to run it in the background\n' "$bold" "$DATA_DIR" "$reset"
  fi
  printf '  Check your setup any time with: fly-telegram --data-dir "%s" doctor\n\n' "$DATA_DIR"

  if [ "$START" -eq 1 ] && [ "$SERVICE" -eq 0 ]; then
    exec "$BIN_DIR/fly-telegram" --data-dir "$DATA_DIR"
  fi
}

main
