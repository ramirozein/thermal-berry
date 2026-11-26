#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
fixture=$(mktemp -d)
export GNUPGHOME="$fixture/keys"
cleanup() {
    gpgconf --kill gpg-agent || true
    rm -rf -- "$fixture"
}
trap cleanup EXIT
cd "$fixture"
mkdir -m 700 "$GNUPGHOME"
gpg --batch --passphrase '' --quick-generate-key 'Thermal Berry fixture <fixture@example.invalid>' ed25519 sign 1d
keys=$(gpg --batch --with-colons --list-keys)
THERMAL_BERRY_RELEASE_FINGERPRINT=$(awk -F: '$1=="fpr" {print $10;exit}' <<< "$keys")
THERMAL_BERRY_RELEASE_SIGNING_KEY=$(gpg --batch --armor --export-secret-keys "$THERMAL_BERRY_RELEASE_FINGERPRINT")
THERMAL_BERRY_RELEASE_PUBLIC_KEY=$(gpg --batch --armor --export "$THERMAL_BERRY_RELEASE_FINGERPRINT")
export THERMAL_BERRY_RELEASE_FINGERPRINT THERMAL_BERRY_RELEASE_SIGNING_KEY THERMAL_BERRY_RELEASE_PUBLIC_KEY

signer="$root/scripts/sign-release.sh"
public_key=$(bash "$signer" --public-key)
[[ $public_key == "$THERMAL_BERRY_RELEASE_PUBLIC_KEY" ]]
for argument in --public-key v1.2.0; do
    if THERMAL_BERRY_RELEASE_PUBLIC_KEY="$public_key"$'\n'"$THERMAL_BERRY_RELEASE_SIGNING_KEY" \
        bash "$signer" "$argument" > public.out 2> public.err; then
        echo 'Accepted secret key material in the public key input.' >&2
        exit 1
    fi
    grep -Fq 'Expected a single public key without secret key material.' public.err
    [[ ! -s public.out && ! -e release-keyring.gpg ]]
done

mkdir -p demo/DEBIAN packages/thermal-berry-{linux,windows,macos}
printf '%s\n' 'Package: thermal-berry' 'Version: 1.2.0' 'Architecture: amd64' \
    'Maintainer: Fixture <fixture@example.invalid>' 'Description: Synthetic test package' > demo/DEBIAN/control
dpkg-deb --build --root-owner-group demo packages/thermal-berry-linux/thermal-berry_1.2.0_amd64.deb
printf 'replacement\n' > packages/thermal-berry-linux/fixture.AppImage
touch packages/thermal-berry-linux/{Z-order.AppImage,fixture.rpm} \
    packages/thermal-berry-windows/fixture-setup.exe packages/thermal-berry-macos/fixture.dmg
cp "$root/install.sh" .
bash "$signer" v1.2.0

(
    cd signed
    {
        printf '# Product: thermal-berry\n# Version: v1.2.0\n'
        sha512sum -- *.AppImage *.rpm *-setup.exe *.dmg install.sh
    } | cmp - SHA512SUMS
)
grep -qx 'Origin: Thermal Berry' pages/apt/dists/stable/Release
grep -qx 'Version: 1.2.0' pages/apt/dists/stable/main/binary-amd64/Packages
# Keep APT configuration, lock files, lists, and caches in the fixture.
mkdir -p apt/empty apt/lists apt/archives
touch apt/status
printf 'deb [arch=amd64 signed-by=%s] file:%s stable main\n' \
    "$fixture/release-keyring.gpg" "$fixture/pages/apt" > apt/sources.list
cat > apt/config <<APT
Dir::Etc::main "/dev/null";
Dir::Etc::parts "$fixture/apt/empty";
Dir::Etc::sourcelist "$fixture/apt/sources.list";
Dir::Etc::sourceparts "$fixture/apt/empty";
Dir::State::status "$fixture/apt/status";
Dir::State::lists "$fixture/apt/lists";
Dir::Cache::archives "$fixture/apt/archives";
Dir::Cache::pkgcache "";
Dir::Cache::srcpkgcache "";
APT
APT_CONFIG="$fixture/apt/config" apt-get update
APT_CONFIG="$fixture/apt/config" apt-cache policy thermal-berry > apt/policy
grep -q 'Candidate: 1.2.0' apt/policy
rm -f apt/lists/*_InRelease
rm pages/apt/dists/stable/InRelease
if APT_CONFIG="$fixture/apt/config" apt-get update > apt/result 2>&1; then
    echo "APT accepted a repository without its signature" >&2
    exit 1
fi
grep -qi 'not signed' apt/result

unset THERMAL_BERRY_RELEASE_SIGNING_KEY THERMAL_BERRY_RELEASE_PUBLIC_KEY THERMAL_BERRY_RELEASE_KEYRING
export BERRY_FIXTURE="$fixture"
mkdir mock-bin
cat > mock-bin/curl <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
url=
destination=
while (($#)); do
    case $1 in
        https://*) url=$1 ;;
        -o) shift; destination=$1 ;;
    esac
    shift
done
case $url in
    https://api.github.com/*)
        tag=${url##*/}
        if [[ $tag == latest ]]; then tag=v1.2.0; fi
        base="https://github.com/ramirozein/thermal-berry/releases/download/$tag"
        printf '{\n  "tag_name": "%s",\n  "assets": [\n' "$tag"
        printf '    {"browser_download_url": "%s/fixture.AppImage"},\n' "$base"
        printf '    {"browser_download_url": "%s/SHA512SUMS"},\n' "$base"
        printf '    {"browser_download_url": "%s/SHA512SUMS.sig"}\n  ]\n}\n' "$base"
        ;;
    https://github.com/*) cp -- "$BERRY_FIXTURE/signed/${url##*/}" "$destination" ;;
    *) exit 1 ;;
esac
MOCK
chmod +x mock-bin/curl
export PATH="$fixture/mock-bin:$PATH"
printf 'original\n' > installed
THERMAL_BERRY_RELEASE_KEYRING="$fixture/release-keyring.gpg" \
    bash install.sh --update-appimage "$fixture/installed" v1.2.0
cmp installed signed/fixture.AppImage
printf 'original\n' > installed
installer() {
    bash pages/install.sh --update-appimage "$fixture/installed" "$1"
}
reject() {
    local version=$1 error=$2
    if installer "$version" >result 2>&1; then
        echo "Installer accepted an invalid update" >&2
        exit 1
    fi
    grep -Fq "$error" result
    grep -qx replacement installed
}

installer v1.2.0
cmp installed signed/fixture.AppImage
reject v1.3.0 "does not match the requested version"
printf 'modified\n' > signed/fixture.AppImage
reject v1.2.0 "checksum did not match"
printf 'invalid\n' > signed/SHA512SUMS.sig
reject v1.2.0 "signature is invalid"
rm signed/SHA512SUMS.sig
reject v1.2.0 "Could not download release signature"
