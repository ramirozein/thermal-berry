#!/usr/bin/env bash
# Installs the latest Thermal Berry release from GitHub Releases.
# Usage: curl -fsSL https://ramirozein.me/thermal-berry/install.sh | bash
set -euo pipefail

readonly REPO="ramirozein/thermal-berry"
readonly BIN_NAME="thermal-berry"
readonly INSTALL_DIR="${THERMAL_BERRY_INSTALL_DIR:-$HOME/.local/bin}"
readonly APP_DIR="$HOME/.local/share/applications"

RELEASE_JSON=""
WORK_DIR=""

log() {
    printf '\033[1;32m==>\033[0m %s\n' "$1"
}

die() {
    printf '\033[1;31mError:\033[0m %s\n' "$1" >&2
    exit 1
}

cleanup() {
    if [[ -n "$WORK_DIR" ]]; then
        rm -rf -- "$WORK_DIR"
    fi
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "$1 is required"
}

check_architecture() {
    local architecture
    architecture="$(uname -m)"

    # Alienware laptops are x86_64 only.
    [[ "$architecture" == "x86_64" ]] \
        || die "Unsupported architecture: $architecture (Thermal Berry targets x86_64 Alienware laptops)"
}

fetch_release() {
    log "Fetching latest release info..."
    RELEASE_JSON="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest")" \
        || die "Could not reach the GitHub API"
}

asset_url() {
    local pattern="$1"
    local url

    while IFS= read -r url || [[ -n "$url" ]]; do
        if [[ "$url" =~ $pattern ]]; then
            printf '%s\n' "$url"
            return 0
        fi
    done < <(
        printf '%s' "$RELEASE_JSON" \
            | grep -oE '"browser_download_url":[[:space:]]*"[^"]+"' \
            | sed -E 's/.*"([^"]+)"$/\1/'
    )

    return 1
}

download_file() {
    local url="$1"
    local destination="$2"
    local description="$3"

    curl -fsSL "$url" -o "$destination" \
        || die "Could not download $description"
}

verify_checksum() {
    local file="$1"
    local name="$2"
    local sums_file="$WORK_DIR/SHA256SUMS"
    local sums_url expected actual

    if [[ ! -f "$sums_file" ]]; then
        sums_url="$(asset_url 'SHA256SUMS$')" \
            || die "No SHA256SUMS asset found in the latest release; refusing to install an unverified binary"
        download_file "$sums_url" "$sums_file" "SHA256SUMS"
    fi

    expected="$(awk -v name="$name" '$2 == name { print $1; exit }' "$sums_file")"
    [[ -n "$expected" ]] \
        || die "No checksum entry found for $name in SHA256SUMS"

    actual="$(sha256sum "$file" | awk '{ print $1 }')" \
        || die "Could not calculate the checksum for $name"
    [[ "$expected" == "$actual" ]] \
        || die "Checksum verification failed for $name (expected $expected, got $actual) - possible tampering, aborting"
}

download_and_verify() {
    local url="$1"
    local destination="$2"
    local description="$3"
    local name="${url##*/}"

    log "Downloading $description..."
    download_file "$url" "$destination" "$description"
    log "Verifying checksum..."
    verify_checksum "$destination" "$name"
}

install_deb() {
    local url="$1"
    local package="$WORK_DIR/package.deb"
    local status

    download_and_verify "$url" "$package" ".deb package"
    require_command sudo
    require_command dpkg-query
    log "Installing (sudo required)..."

    if ! sudo dpkg -i "$package"; then
        require_command apt-get
        sudo apt-get install -f -y \
            || die "Could not repair .deb package dependencies"
        sudo dpkg -i "$package" \
            || die "Could not install the .deb package after repairing dependencies"
    fi

    status="$(dpkg-query -W -f='${Status}' "$BIN_NAME" 2>/dev/null)" \
        || die "Could not verify installation of $BIN_NAME"
    [[ "$status" == "install ok installed" ]] \
        || die "$BIN_NAME is not fully installed (status: $status)"
}

install_appimage() {
    local url="$1"
    local appimage="$WORK_DIR/$BIN_NAME.AppImage"

    mkdir -p "$INSTALL_DIR" "$APP_DIR"
    download_and_verify "$url" "$appimage" "AppImage"
    mv "$appimage" "$INSTALL_DIR/$BIN_NAME"
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

main() {
    local package_url

    require_command curl
    require_command sha256sum
    require_command grep
    require_command sed
    require_command awk
    check_architecture

    WORK_DIR="$(mktemp -d)" || die "Could not create a temporary directory"
    trap cleanup EXIT

    fetch_release

    if command -v dpkg >/dev/null 2>&1; then
        if package_url="$(asset_url 'amd64\.deb$')"; then
            log "apt/dpkg detected, installing .deb package"
            install_deb "$package_url"
        else
            log ".deb asset not available, falling back to AppImage"
            package_url="$(asset_url '\.AppImage$')" \
                || die "No AppImage asset found in the latest release"
            install_appimage "$package_url"
        fi
    else
        log "No dpkg found, installing portable AppImage"
        package_url="$(asset_url '\.AppImage$')" \
            || die "No AppImage asset found in the latest release"
        install_appimage "$package_url"
    fi

    log "Thermal Berry installed. Launch it from your applications menu."
}

main "$@"
