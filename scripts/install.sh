#!/bin/sh
# 0x CLI installer.
#
#   curl -fsSL https://raw.githubusercontent.com/0xProject/0x-cli/main/scripts/install.sh | sh
#
# Downloads a prebuilt `0x` binary from GitHub Releases, verifies its SHA-256
# checksum, and installs it to a bin directory on your PATH.
#
# Knobs (all optional, set as environment variables):
#   ZEROX_VERSION   version to install, e.g. v0.1.0   (default: latest release)
#   ZEROX_BIN_DIR   install directory                 (default: ~/.local/bin)
#
# Examples:
#   curl -fsSL .../install.sh | sh
#   curl -fsSL .../install.sh | ZEROX_VERSION=v0.1.0 sh
#   curl -fsSL .../install.sh | ZEROX_BIN_DIR=/usr/local/bin sh

set -eu

REPO="0xProject/0x-cli"
BIN="0x"

# --- pretty output (no color when not a tty) -------------------------------
if [ -t 2 ]; then
  BOLD="$(printf '\033[1m')"; RED="$(printf '\033[31m')"
  GREEN="$(printf '\033[32m')"; DIM="$(printf '\033[2m')"; RESET="$(printf '\033[0m')"
else
  BOLD=""; RED=""; GREEN=""; DIM=""; RESET=""
fi
info() { printf '%s\n' "$*" >&2; }
warn() { printf '%swarning:%s %s\n' "$RED" "$RESET" "$*" >&2; }
err()  { printf '%serror:%s %s\n'   "$RED" "$RESET" "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || err "required command not found: $1"; }

# --- pick a downloader ------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  DL="curl"
elif command -v wget >/dev/null 2>&1; then
  DL="wget"
else
  err "need either curl or wget installed"
fi

# fetch URL to stdout
fetch() {
  if [ "$DL" = curl ]; then
    curl -fsSL "$1"
  else
    wget -qO- "$1"
  fi
}
# download URL to a file; returns non-zero on HTTP error
download() {
  # $1 url  $2 dest
  if [ "$DL" = curl ]; then
    curl -fsSL -o "$2" "$1"
  else
    wget -qO "$2" "$1"
  fi
}

# --- detect platform --------------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux)  os_name="unknown-linux-gnu"; ext="tar.gz" ;;
  Darwin) os_name="apple-darwin";      ext="tar.gz" ;;
  MINGW*|MSYS*|CYGWIN*)
    err "this script targets macOS and Linux. On Windows, download the .zip from:
  https://github.com/$REPO/releases/latest" ;;
  *) err "unsupported OS: $os" ;;
esac

case "$arch" in
  x86_64|amd64)        cpu="x86_64" ;;
  arm64|aarch64)       cpu="aarch64" ;;
  *) err "unsupported architecture: $arch" ;;
esac

target="${cpu}-${os_name}"

# --- resolve version --------------------------------------------------------
version="${ZEROX_VERSION:-}"
if [ -z "$version" ]; then
  info "${DIM}Resolving latest release...${RESET}"
  # The /releases/latest page 302-redirects to /releases/tag/<version>; parse
  # the Location header. Avoids the GitHub API's unauthenticated rate limit.
  if [ "$DL" = curl ]; then
    loc="$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
      "https://github.com/$REPO/releases/latest")"
  else
    loc="$(wget --max-redirect=0 -S -O /dev/null \
      "https://github.com/$REPO/releases/latest" 2>&1 \
      | awk '/^[[:space:]]*Location:/ {print $2}' | tail -n1)"
  fi
  version="${loc##*/}"
  [ -n "$version" ] || err "could not determine the latest version; set ZEROX_VERSION=vX.Y.Z"
fi
case "$version" in v*) ;; *) version="v$version" ;; esac

# --- build URLs -------------------------------------------------------------
asset="${BIN}-${target}.${ext}"
base="https://github.com/$REPO/releases/download/$version"
asset_url="$base/$asset"
sum_url="$asset_url.sha256"

info "${BOLD}Installing $BIN $version${RESET} ${DIM}($target)${RESET}"

# --- download into a temp dir ----------------------------------------------
tmp="$(mktemp -d 2>/dev/null || mktemp -d -t zeroxcli)"
trap 'rm -rf "$tmp"' EXIT INT TERM

if ! download "$asset_url" "$tmp/$asset"; then
  err "download failed: $asset_url
The release may not have a build for your platform ($target),
or the version may not exist. See https://github.com/$REPO/releases"
fi

# --- verify checksum (best-effort; warn if checksum asset is missing) -------
if download "$sum_url" "$tmp/$asset.sha256" 2>/dev/null; then
  expected="$(awk '{print $1}' "$tmp/$asset.sha256")"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
  else
    actual=""; warn "no sha256sum/shasum found; skipping checksum verification"
  fi
  if [ -n "$actual" ]; then
    [ "$actual" = "$expected" ] || err "checksum mismatch for $asset
  expected: $expected
  actual:   $actual"
    info "${DIM}Checksum verified.${RESET}"
  fi
else
  warn "no checksum file published for $asset; skipping verification"
fi

# --- extract ----------------------------------------------------------------
need tar
tar -xzf "$tmp/$asset" -C "$tmp"
# Archive contains a directory "0x-<target>/" holding the binary.
binpath="$tmp/${BIN}-${target}/${BIN}"
[ -f "$binpath" ] || binpath="$(find "$tmp" -type f -name "$BIN" -perm -u+x 2>/dev/null | head -n1)"
[ -f "$binpath" ] || err "could not find the $BIN binary inside $asset"
chmod +x "$binpath"

# --- choose install dir -----------------------------------------------------
bin_dir="${ZEROX_BIN_DIR:-$HOME/.local/bin}"
mkdir -p "$bin_dir" 2>/dev/null || err "cannot create install dir: $bin_dir"
if [ ! -w "$bin_dir" ]; then
  err "install dir is not writable: $bin_dir
Re-run with a writable dir, e.g.  ZEROX_BIN_DIR=\$HOME/.local/bin
or with elevated permissions for a system path."
fi

dest="$bin_dir/$BIN"
mv -f "$binpath" "$dest"
info "${GREEN}Installed${RESET} $dest"

# --- PATH hint --------------------------------------------------------------
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *)
    info ""
    warn "$bin_dir is not on your PATH."
    info "Add it by appending this line to your shell profile (e.g. ~/.zshrc, ~/.bashrc):"
    info "  ${BOLD}export PATH=\"$bin_dir:\$PATH\"${RESET}"
    ;;
esac

# --- verify -----------------------------------------------------------------
info ""
if "$dest" --version >/dev/null 2>&1; then
  info "${GREEN}Done.${RESET} Run ${BOLD}$BIN --version${RESET} to get started."
  "$dest" --version >&2 || true
else
  info "${GREEN}Done.${RESET} Installed to $dest"
fi
