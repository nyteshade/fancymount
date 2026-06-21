#!/usr/bin/env bash
# ┌──────────────────────────────────────────────────────────────┐
# │  scripts/install.sh — one-shot installer for fancymount     │
# └──────────────────────────────────────────────────────────────┘
#
# Downloads the latest fm binary from GitHub Releases and installs
# it into the first writable directory in the PATH-like search list:
#   ~/.local/bin  →  ~/bin  →  /usr/local/bin  →  (prompt)
#
# On macOS, strips the com.apple.quarantine extended attribute
# that Safari / Chrome attach to internet downloads (otherwise
# Gatekeeper will block execution).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/nyteshade/fancymount/main/scripts/install.sh | bash
#
#   # Or pin a version:
#   curl -fsSL ... | bash -s v1.0.0

set -euo pipefail

# ── Configuration ──────────────────────────────────────────────

REPO="nyteshade/fancymount"
BIN_NAME="fm"
VERSION="${1:-latest}"

# ── Colour helpers ─────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Colour

info()  { printf "${CYAN}→${NC} %s\n" "$*"; }
ok()    { printf "${GREEN}✓${NC} %s\n" "$*"; }
warn()  { printf "${YELLOW}⚠${NC} %s\n" "$*"; }
err()   { printf "${RED}✗${NC} %s\n" "$*"; exit 1; }

# ── Detect platform ────────────────────────────────────────────

OS=$(uname -s)
ARCH=$(uname -m)

case "$OS" in
  Darwin)
    case "$ARCH" in
      arm64)  ASSET_PATTERN="macos-universal" ;;  # universal runs on both
      x86_64) ASSET_PATTERN="macos-universal" ;;
      *)      err "Unsupported macOS architecture: $ARCH" ;;
    esac
    ;;
  Linux)
    # We'll use the universal macOS binary as a placeholder;
    # Linux cross-compilation needs a cross-linker not set up here.
    # When Linux binaries are published, add a case for them.
    case "$ARCH" in
      x86_64) ASSET_PATTERN="linux-x86_64" ;;
      aarch64) ASSET_PATTERN="linux-aarch64" ;;
      *)      err "Unsupported Linux architecture: $ARCH" ;;
    esac
    ;;
  *) err "Unsupported OS: $OS" ;;
esac

info "Detected: ${OS} ${ARCH}"

# ── Download ───────────────────────────────────────────────────

if [ "$VERSION" = "latest" ]; then
  RELEASE_URL="https://github.com/${REPO}/releases/latest/download"
else
  RELEASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

info "Looking for asset matching: *${ASSET_PATTERN}*"

# Get the download URL from the GitHub API
if [ "$VERSION" = "latest" ]; then
  API_URL="https://api.github.com/repos/${REPO}/releases/latest"
else
  API_URL="https://api.github.com/repos/${REPO}/releases/tags/${VERSION}"
fi

DOWNLOAD_URL=$(curl -fsSL "$API_URL" \
  | grep -o "\"browser_download_url\": \"[^\"]*${ASSET_PATTERN}[^\"]*\"" \
  | head -1 \
  | sed 's/.*"\(https:.*\)".*/\1/')

if [ -z "$DOWNLOAD_URL" ]; then
  err "Could not find a release asset for ${OS} ${ARCH}.  Check https://github.com/${REPO}/releases"
fi

info "Downloading ${DOWNLOAD_URL}..."
curl -fsSL -o "${TMPDIR}/${BIN_NAME}.tar.gz" "$DOWNLOAD_URL"

info "Extracting..."
tar -xzf "${TMPDIR}/${BIN_NAME}.tar.gz" -C "$TMPDIR"

if [ ! -f "${TMPDIR}/${BIN_NAME}" ]; then
  err "Binary not found in archive (expected '${BIN_NAME}')"
fi

chmod +x "${TMPDIR}/${BIN_NAME}"

# ── macOS: strip quarantine xattr ──────────────────────────────

if [ "$OS" = "Darwin" ]; then
  xattr -d com.apple.quarantine "${TMPDIR}/${BIN_NAME}" 2>/dev/null || true
fi

# ── Find install directory ─────────────────────────────────────

install_dir=""

for dir in "$HOME/.local/bin" "$HOME/bin" "/usr/local/bin"; do
  if [ -d "$dir" ] && [ -w "$dir" ]; then
    install_dir="$dir"
    break
  fi
  # Create ~/.local/bin if it doesn't exist
  if [ "$dir" = "$HOME/.local/bin" ] && [ ! -d "$dir" ]; then
    mkdir -p "$dir" 2>/dev/null && install_dir="$dir" && break
  fi
done

if [ -z "$install_dir" ]; then
  warn "No writable directory found in ~/.local/bin, ~/bin, or /usr/local/bin."
  printf "  Where should I install ${BIN_NAME}? "
  read -r install_dir
  if [ -z "$install_dir" ]; then
    err "No directory provided.  Aborting."
  fi
  mkdir -p "$install_dir" 2>/dev/null || err "Cannot create ${install_dir}"
fi

# ── Install ────────────────────────────────────────────────────

if [ -f "${install_dir}/${BIN_NAME}" ]; then
  info "Replacing existing ${install_dir}/${BIN_NAME}"
fi

cp "${TMPDIR}/${BIN_NAME}" "${install_dir}/${BIN_NAME}"

# macOS: strip quarantine from installed location too
if [ "$OS" = "Darwin" ]; then
  xattr -d com.apple.quarantine "${install_dir}/${BIN_NAME}" 2>/dev/null || true
fi

# ── Verify PATH ────────────────────────────────────────────────

if ! echo "$PATH" | tr ':' '\n' | grep -qxF "$install_dir"; then
  warn "${install_dir} is not on your PATH."
  printf "  Add this to your shell profile (~/.zshrc or ~/.bashrc):\n"
  printf "  ${CYAN}export PATH=\"%s:\$PATH\"${NC}\n" "$install_dir"
fi

# ── Done ───────────────────────────────────────────────────────

echo ""
ok "fancymount installed to ${install_dir}/${BIN_NAME}"
echo ""
echo "  Run it:  ${GREEN}fm${NC}"
echo "  Or alias: ${GREEN}alias mount='fm'${NC}"
echo ""
