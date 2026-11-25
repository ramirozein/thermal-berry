#!/usr/bin/env bash
set -euo pipefail

readonly REPO="ramirozein/thermal-berry"
readonly BIN_NAME="thermal-berry"
readonly INSTALL_DIR="${THERMAL_BERRY_INSTALL_DIR:-$HOME/.local/bin}"
readonly APP_DIR="$HOME/.local/share/applications"
readonly EMBEDDED_RELEASE_KEYRING=""

RELEASE_JSON=""
WORK_DIR=""
STAGED_APPIMAGE=""
UPDATE_METHOD=""
RELEASE_MANIFEST=""
APPIMAGE_TARGET=""
RELEASE_TAG=""

log() {
    printf '\033[1;32m==>\033[0m %s\n' "$1"
}

die() {
    printf 'Error: %s\n' "$1" >&2
    exit 1
}

cleanup() {
    if [[ -n "$STAGED_APPIMAGE" ]]; then
        rm -f -- "$STAGED_APPIMAGE"
    fi
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

    [[ "$architecture" == "x86_64" ]] \
        || die "Unsupported architecture: $architecture (Thermal Berry targets x86_64 Alienware laptops)"
}

fetch_release() {
    local endpoint="latest"
    if [[ -n "$RELEASE_TAG" ]]; then
        endpoint="tags/$RELEASE_TAG"
    fi
    log "Fetching release info..."
    RELEASE_JSON="$(curl -q -fsSL --proto '=https' --proto-redir '=https' --connect-timeout 15 --max-time 60 "https://api.github.com/repos/${REPO}/releases/$endpoint")" \
        || die "Could not reach the GitHub API"
}

asset_url() {
    local pattern="$1"
    local url

    while IFS= read -r url || [[ -n "$url" ]]; do
        if [[ "$url" == "https://github.com/$REPO/releases/download/$RELEASE_TAG/"* && "$url" =~ $pattern ]]; then
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

    curl -q -fsSL --proto '=https' --proto-redir '=https' --connect-timeout 15 --max-time 600 "$url" -o "$destination" \
        || die "Could not download $description"
}

verify_release_manifest() {
    local manifest_url signature_url keyring gpg_home
    manifest_url="$(asset_url '/SHA512SUMS$')" || die "The release has no signed manifest"
    signature_url="$(asset_url '/SHA512SUMS\.sig$')" || die "The release has no manifest signature"
    keyring="${THERMAL_BERRY_RELEASE_KEYRING:-}"
    if [[ -n "${THERMAL_BERRY_RELEASE_PUBLIC_KEY:-}" ]]; then
        require_command gpg
        gpg_home="$WORK_DIR/gnupg"
        mkdir -m 700 -- "$gpg_home"
        keyring="$WORK_DIR/release.gpg"
        printf '%s\n' "$THERMAL_BERRY_RELEASE_PUBLIC_KEY" |
            gpg --batch --homedir "$gpg_home" --dearmor --output "$keyring"
    elif [[ -z "$keyring" && -n "$EMBEDDED_RELEASE_KEYRING" ]]; then
        require_command base64
        keyring="$WORK_DIR/release.gpg"
        printf '%s' "$EMBEDDED_RELEASE_KEYRING" | base64 --decode > "$keyring" ||
            die "Could not decode the embedded release keyring"
    fi
    [[ "$keyring" == /* && -f "$keyring" ]] ||
        die "Set THERMAL_BERRY_RELEASE_KEYRING to the absolute path of a verified public keyring"
    RELEASE_MANIFEST="$WORK_DIR/SHA512SUMS"
    download_file "$manifest_url" "$RELEASE_MANIFEST" "release manifest"
    download_file "$signature_url" "$WORK_DIR/SHA512SUMS.sig" "release signature"
    mkdir -m 700 -- "$WORK_DIR/verify"
    gpgv --homedir "$WORK_DIR/verify" --keyring "$keyring" "$WORK_DIR/SHA512SUMS.sig" "$RELEASE_MANIFEST" ||
        die "The release manifest signature is invalid"
    [[ "$(sed -n '1p' "$RELEASE_MANIFEST")" == "# Product: $BIN_NAME" &&
       "$(sed -n '2p' "$RELEASE_MANIFEST")" == "# Version: $RELEASE_TAG" ]] ||
        die "The signed manifest does not match the requested version"
}

download_and_verify() {
    local url="$1" destination="$2" name="${1##*/}" expected actual
    [[ "$name" =~ ^[a-zA-Z0-9._+-]+\.AppImage$ ]] || die "Invalid AppImage asset name"
    verify_release_manifest
    expected="$(awk -v name="$name" '$2 == name { print $1; count++ } END { if (count != 1) exit 1 }' "$RELEASE_MANIFEST")" ||
        die "The manifest must identify exactly one checksum for $name"
    [[ "$expected" =~ ^[0-9a-f]{128}$ ]] || die "Invalid SHA-512 checksum"
    download_file "$url" "$destination" "AppImage"
    actual="$(sha512sum "$destination" | awk '{ print $1 }')"
    [[ "$expected" == "$actual" ]] || die "AppImage checksum did not match the signed manifest"
}

install_appimage() {
    local url="$1"
    local appimage="$WORK_DIR/$BIN_NAME.AppImage"

    local destination="${APPIMAGE_TARGET:-$INSTALL_DIR/$BIN_NAME}"
    local destination_dir
    destination_dir="$(dirname -- "$destination")"

    mkdir -p "$destination_dir"
    [[ -w "$destination_dir" ]] || die "Cannot update AppImage: $destination_dir is not writable"
    download_and_verify "$url" "$appimage"
    STAGED_APPIMAGE="$(mktemp "$destination_dir/.thermal-berry-update.XXXXXX")"
    cp -- "$appimage" "$STAGED_APPIMAGE"
    chmod 755 "$STAGED_APPIMAGE"
    mv -Tf -- "$STAGED_APPIMAGE" "$destination"
    STAGED_APPIMAGE=""

    [[ "$UPDATE_METHOD" == "appimage" ]] && return 0

    mkdir -p "$APP_DIR"

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

    case "${1:-}" in
        --update-deb) die "Install and maintain the Debian package via the signed APT repository" ;;
        --update-appimage)
            [[ $# -eq 3 ]] || die "Usage: --update-appimage PATH RELEASE_TAG"
            UPDATE_METHOD="appimage"
            APPIMAGE_TARGET="$(readlink -f -- "$2")"
            [[ -f "$APPIMAGE_TARGET" ]] || die "Installed AppImage not found: $2"
            RELEASE_TAG="$3"
            ;;
        "") ;;
        *) die "Unknown argument: $1" ;;
    esac
    if [[ -n "$UPDATE_METHOD" ]]; then
        [[ "$RELEASE_TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "Invalid release tag: $RELEASE_TAG"
    fi

    require_command curl
    require_command gpgv
    require_command sha512sum
    require_command grep
    require_command sed
    require_command awk
    check_architecture

    WORK_DIR="$(mktemp -d)" || die "Could not create a temporary directory"
    trap cleanup EXIT

    fetch_release

    if [[ -z "$RELEASE_TAG" ]]; then
        RELEASE_TAG="$(sed -nE 's/^[[:space:]]*"tag_name":[[:space:]]*"(v[0-9]+\.[0-9]+\.[0-9]+)".*/\1/p' <<< "$RELEASE_JSON")"
        [[ "$RELEASE_TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "Could not determine a stable release version"
    fi
    package_url="$(asset_url '\.AppImage$')" || die "No AppImage asset found for $RELEASE_TAG"
    install_appimage "$package_url"

    log "Thermal Berry installed. Launch it from your applications menu."
}

main "$@"
