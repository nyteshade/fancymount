#!/usr/bin/env bash
# ┌──────────────────────────────────────────────────────────────┐
# │  scripts/release.sh — build + tag + publish GitHub release  │
# └──────────────────────────────────────────────────────────────┘
#
# Usage:
#   ./scripts/release.sh                 # bump patch, build, publish
#   ./scripts/release.sh minor           # bump minor
#   ./scripts/release.sh 1.2.3           # exact version
#
# Prerequisites:
#   - Rust toolchain with aarch64 + x86_64 macOS targets
#   - gh CLI (brew install gh) + authenticated (gh auth login)
#   - lipo (included with Xcode / Command Line Tools)

set -euo pipefail
cd "$(dirname "$0")/.."

# ── Determine version ──────────────────────────────────────────

current=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')

if [ $# -eq 0 ]; then
  # Bump patch by default
  IFS='.' read -r maj min patch <<< "$current"
  version="${maj}.${min}.$((patch + 1))"
elif [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  version="$1"
else
  # "minor" or "major" — crude bump
  IFS='.' read -r maj min patch <<< "$current"
  case "$1" in
    major) version="$((maj + 1)).0.0" ;;
    minor) version="${maj}.$((min + 1)).0" ;;
    *)     echo "Unknown bump: $1"; exit 1 ;;
  esac
fi

echo "→ Building fancymount v${version} (current: v${current})"

# ── Update Cargo.toml version ──────────────────────────────────
sed -i '' "s/^version = \"${current}\"/version = \"${version}\"/" Cargo.toml

# ── Build ──────────────────────────────────────────────────────
echo "→ Building aarch64-apple-darwin..."
cargo build --release --target aarch64-apple-darwin

echo "→ Building x86_64-apple-darwin..."
cargo build --release --target x86_64-apple-darwin

# ── Universal binary ───────────────────────────────────────────
echo "→ Creating universal binary..."
lipo -create \
  -output target/release/fm \
  target/aarch64-apple-darwin/release/fm \
  target/x86_64-apple-darwin/release/fm

echo "→ Stripping..."
strip target/release/fm 2>/dev/null || true

# ── Package ────────────────────────────────────────────────────
tarball="fancymount-v${version}-macos-universal.tar.gz"
echo "→ Packaging ${tarball}..."
tar -czf "target/${tarball}" -C target/release fm

shasum -a 256 "target/${tarball}" > "target/${tarball}.sha256"

# ── Commit version bump ────────────────────────────────────────
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to v${version}" || true

# ── Tag & push ─────────────────────────────────────────────────
git tag "v${version}"
git push origin main
git push origin "v${version}"

# ── GitHub Release ─────────────────────────────────────────────
echo "→ Creating GitHub release v${version}..."
gh release create "v${version}" \
  "target/${tarball}" \
  "target/${tarball}.sha256" \
  --title "fancymount v${version}" \
  --notes "$(cat <<NOTES
## fancymount v${version}

A terminal UI mount manager for macOS and Linux.

### Install

\`\`\`bash
curl -fsSL https://raw.githubusercontent.com/nyteshade/fancymount/main/scripts/install.sh | bash
\`\`\`

### Manual download

Download \`${tarball}\`, extract, and copy \`fm\` to your PATH.

\`\`\`bash
tar -xzf ${tarball}
sudo cp fm /usr/local/bin/
# macOS: clear quarantine flag from internet download
sudo xattr -d com.apple.quarantine /usr/local/bin/fm 2>/dev/null || true
\`\`\`
NOTES
)"

echo ""
echo "✅ Released fancymount v${version}"
echo "   ${tarball}"
echo "   https://github.com/nyteshade/fancymount/releases/tag/v${version}"
