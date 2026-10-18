#!/bin/sh
# Installs a prebuilt oshi-mcp binary from GitHub Releases after checking it
# against the release's SHA256SUMS.
#
#   curl -fsSL https://raw.githubusercontent.com/johnzilla/oshi-mcp/main/install.sh | sh
#
# Environment:
#   OSHI_MCP_VERSION      release tag to install, e.g. v0.1.0 (default: latest)
#   OSHI_MCP_INSTALL_DIR  where to put the binary (default: ~/.local/bin)

set -eu

REPO="johnzilla/oshi-mcp"
BIN="oshi-mcp"

fail() {
  echo "error: $*" >&2
  exit 1
}

# Everything runs inside main so a truncated download never executes half a script.
main() {
  version="${OSHI_MCP_VERSION:-latest}"
  install_dir="${OSHI_MCP_INSTALL_DIR:-$HOME/.local/bin}"

  case "$(uname -s)" in
    Linux) os="unknown-linux-musl" ;;
    Darwin) os="apple-darwin" ;;
    *) fail "unsupported OS $(uname -s); on Windows download the .exe from https://github.com/$REPO/releases" ;;
  esac
  case "$(uname -m)" in
    x86_64 | amd64) arch="x86_64" ;;
    aarch64 | arm64) arch="aarch64" ;;
    *) fail "unsupported architecture $(uname -m)" ;;
  esac
  asset="$BIN-$arch-$os"

  if [ "$version" = "latest" ]; then
    base="https://github.com/$REPO/releases/latest/download"
  else
    base="https://github.com/$REPO/releases/download/$version"
  fi

  if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d' ' -f1; }
  elif command -v shasum >/dev/null 2>&1; then
    sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
  else
    fail "need sha256sum or shasum to verify the download"
  fi

  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT INT TERM

  echo "Downloading $asset ($version)"
  fetch() { curl --proto '=https' --tlsv1.2 -fsSL "$1" -o "$2"; }
  fetch "$base/$asset" "$tmp/$asset" || fail "download failed: $base/$asset"
  fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || fail "download failed: $base/SHA256SUMS"

  expected=$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
  [ -n "$expected" ] || fail "$asset is not listed in SHA256SUMS"
  actual=$(sha256 "$tmp/$asset")
  [ "$expected" = "$actual" ] || fail "checksum mismatch for $asset (expected $expected, got $actual)"
  echo "Checksum OK"

  mkdir -p "$install_dir"
  chmod 755 "$tmp/$asset"
  mv "$tmp/$asset" "$install_dir/$BIN"
  echo "Installed $("$install_dir/$BIN" --version) to $install_dir/$BIN"

  case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) echo "Note: $install_dir is not on your PATH." ;;
  esac
  if command -v gh >/dev/null 2>&1; then
    echo "Optional provenance check: gh attestation verify $install_dir/$BIN --repo $REPO"
  fi
}

main "$@"
