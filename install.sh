#!/usr/bin/env bash
# Installs the latest Thermal Berry release from GitHub Releases.
# Usage: curl -fsSL https://ramirozein.me/thermal-berry/install.sh | bash
set -euo pipefail

REPO="ramirozein/thermal-berry"
BIN_NAME="thermal-berry"
INSTALL_DIR="${THERMAL_BERRY_INSTALL_DIR:-$HOME/.local/bin}"
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/128x128/apps"

log() { printf '\033[1;32m==>\033[0m %s\n' "$1"; }
die() { printf '\033[1;31mError:\033[0m %s\n' "$1" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || die "curl is required"
command -v sha256sum >/dev/null 2>&1 || die "sha256sum is required"

# Alienware laptops are x86_64 only.
if [ "$(uname -m)" != "x86_64" ]; then
    die "Unsupported architecture: $(uname -m) (Thermal Berry targets x86_64 Alienware laptops)"
fi

log "Fetching latest release info..."
RELEASE_JSON="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest")" \
    || die "Could not reach the GitHub API"

asset_url() {
    printf '%s' "$RELEASE_JSON" \
        | grep -o '"browser_download_url": *"[^"]*"' \
        | sed -E 's/.*"([^"]+)"$/\1/' \
        | grep -E "$1" | head -n1
}

verify_checksum() {
    local file="$1" name="$2" sums_url sums_file expected actual
    sums_url="$(asset_url 'SHA256SUMS$')"
    [ -n "$sums_url" ] || die "No SHA256SUMS asset found in the latest release; refusing to install an unverified binary"
    sums_file="$(mktemp)"
    curl -fsSL "$sums_url" -o "$sums_file" || die "Could not download SHA256SUMS"
    expected="$(grep -F " $name" "$sums_file" | awk '{print $1}')"
    rm -f "$sums_file"
    [ -n "$expected" ] || die "No checksum entry found for $name in SHA256SUMS"
    actual="$(sha256sum "$file" | awk '{print $1}')"
    [ "$expected" = "$actual" ] || die "Checksum verification failed for $name (expected $expected, got $actual) - possible tampering, aborting"
}

install_deb() {
    local url tmp
    url="$(asset_url 'amd64\.deb$')"
    [ -n "$url" ] || return 1
    tmp="$(mktemp --suffix=.deb)"
    log "Downloading .deb package..."
    curl -fsSL "$url" -o "$tmp"
    log "Verifying checksum..."
    verify_checksum "$tmp" "$(basename "$url")"
    log "Installing (sudo required)..."
    sudo dpkg -i "$tmp" || sudo apt-get install -f -y
    rm -f "$tmp"
}

install_appimage() {
    local url tmp
    url="$(asset_url '\.AppImage$')"
    [ -n "$url" ] || die "No AppImage asset found in the latest release"

    mkdir -p "$INSTALL_DIR" "$APP_DIR" "$ICON_DIR"
    tmp="$(mktemp --suffix=.AppImage)"
    log "Downloading AppImage..."
    curl -fsSL "$url" -o "$tmp"
    log "Verifying checksum..."
    verify_checksum "$tmp" "$(basename "$url")"
    mv "$tmp" "$INSTALL_DIR/$BIN_NAME"
    chmod +x "$INSTALL_DIR/$BIN_NAME"

    cat > "$APP_DIR/${BIN_NAME}.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Thermal Berry
Comment=Monitor and control fans/temperature on Alienware laptops
Exec=$INSTALL_DIR/$BIN_NAME
Icon=$BIN_NAME
Categories=Utility;
Terminal=false
EOF

    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *) log "Add $INSTALL_DIR to your PATH to run '$BIN_NAME' from a terminal." ;;
    esac
}

if command -v dpkg >/dev/null 2>&1; then
    log "apt/dpkg detected, installing .deb package"
    install_deb || { log ".deb asset not available, falling back to AppImage"; install_appimage; }
else
    log "No dpkg found, installing portable AppImage"
    install_appimage
fi

log "Thermal Berry installed. Launch it from your applications menu."