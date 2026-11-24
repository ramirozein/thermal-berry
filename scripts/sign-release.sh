#!/usr/bin/env bash
set -euo pipefail

version=${1:?missing release version}
[[ $version =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]
: "${THERMAL_BERRY_RELEASE_PUBLIC_KEY:?missing release public key}"
: "${THERMAL_BERRY_RELEASE_FINGERPRINT:?missing release fingerprint}"
: "${THERMAL_BERRY_RELEASE_SIGNING_KEY:?missing signing key}"
[[ $THERMAL_BERRY_RELEASE_FINGERPRINT =~ ^[0-9A-F]{40}$ ]]

export GNUPGHOME
GNUPGHOME=$(mktemp -d)
trap 'gpgconf --kill gpg-agent || true; rm -rf -- "$GNUPGHOME"' EXIT
printf '%s\n' "$THERMAL_BERRY_RELEASE_PUBLIC_KEY" | gpg --batch --dearmor > release-keyring.gpg
key_info=$(gpg --batch --show-keys --with-colons release-keyring.gpg)
actual=$(awk -F: '$1=="pub" {count++} $1=="fpr" && !fingerprint {fingerprint=$10} END {if (count != 1) exit 1; print fingerprint}' <<< "$key_info")
[[ "$actual" == "$THERMAL_BERRY_RELEASE_FINGERPRINT" ]]
printf '%s\n' "$THERMAL_BERRY_RELEASE_SIGNING_KEY" | gpg --batch --import
sign=(gpg --batch --local-user "$actual" --digest-algo SHA256)

mkdir -p signed "pages/apt/pool/main/t/thermal-berry/${version#v}" pages/apt/dists/stable/main/binary-amd64
cp packages/thermal-berry-linux/*.AppImage packages/thermal-berry-linux/*.rpm \
   packages/thermal-berry-windows/*-setup.exe packages/thermal-berry-macos/*.dmg install.sh signed/
debs=(packages/thermal-berry-linux/*.deb)
[[ ${#debs[@]} == 1 ]]
[[ $(dpkg-deb -f "${debs[0]}" Package) == thermal-berry ]]
[[ $(dpkg-deb -f "${debs[0]}" Version) == "${version#v}" ]]
[[ $(dpkg-deb -f "${debs[0]}" Architecture) == amd64 ]]
cp "${debs[0]}" "pages/apt/pool/main/t/thermal-berry/${version#v}/"

(
    cd signed
    {
        printf '# Product: thermal-berry\n# Version: %s\n' "$version"
        sha512sum -- *.AppImage *.rpm *-setup.exe *.dmg install.sh
    } > SHA512SUMS
    "${sign[@]}" --detach-sign SHA512SUMS
    gpgv --keyring "$OLDPWD/release-keyring.gpg" SHA512SUMS.sig SHA512SUMS
    sha512sum -c SHA512SUMS
)

cp release-keyring.gpg pages/apt/thermal-berry.gpg
(
    cd pages/apt
    index=dists/stable/main/binary-amd64/Packages
    apt-ftparchive packages pool > "$index"
    gzip -n -c "$index" > "$index.gz"
    release=dists/stable/Release
    {
        printf 'Valid-Until: %s\n' "$(date -Ru -d '+365 days')"
        apt-ftparchive -o APT::FTPArchive::Release::Origin='Thermal Berry' \
            -o APT::FTPArchive::Release::Label='Thermal Berry' \
            -o APT::FTPArchive::Release::Suite=stable \
            -o APT::FTPArchive::Release::Codename=stable \
            -o APT::FTPArchive::Release::Architectures=amd64 \
            -o APT::FTPArchive::Release::Components=main \
            release dists/stable
    } > Release.new
    mv Release.new "$release"
    "${sign[@]}" --clearsign --output dists/stable/InRelease "$release"
    gpgv --keyring "$PWD/thermal-berry.gpg" dists/stable/InRelease
)
