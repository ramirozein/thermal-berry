import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const version = process.argv[2];
if (!version || !/^v[0-9]+\.[0-9]+\.[0-9]+$/.test(version)) {
    throw new Error("Expected a release version in vMAJOR.MINOR.PATCH format.");
}
const {
    THERMAL_BERRY_RELEASE_PUBLIC_KEY: publicKey,
    THERMAL_BERRY_RELEASE_FINGERPRINT: fingerprint,
    THERMAL_BERRY_RELEASE_SIGNING_KEY: signingKey,
    ...environment
} = process.env;
if (!publicKey || !signingKey || !fingerprint || !/^[0-9A-F]{40}$/.test(fingerprint)) {
    throw new Error("Expected release keys and a 40-character uppercase fingerprint.");
}

const packageVersion = version.slice(1);
const keyring = resolve("release-keyring.gpg");
const apt = resolve("pages/apt");
const pool = join(apt, "pool/main/t/thermal-berry", packageVersion);
const index = "dists/stable/main/binary-amd64/Packages";
const release = "dists/stable/Release";
const assetGroups = [
    { platform: "linux", suffix: ".AppImage" },
    { platform: "linux", suffix: ".rpm" },
    { platform: "windows", suffix: "-setup.exe" },
    { platform: "macos", suffix: ".dmg" },
];

function assets(directory: string, suffix: string) {
    return readdirSync(directory)
        .filter((name) => !name.startsWith(".") && name.endsWith(suffix))
        .sort((left, right) => Buffer.compare(Buffer.from(left), Buffer.from(right)));
}

const gpgHome = mkdtempSync(join(tmpdir(), "thermal-berry-sign-"));
const env = { ...environment, GNUPGHOME: gpgHome, LC_ALL: "C" };

process.once("SIGINT", () => process.exit(130));
process.once("SIGTERM", () => process.exit(143));

function run(command: string, args: string[], options: { cwd?: string; input?: string } = {}) {
    return execFileSync(command, args, { env, stdio: ["pipe", "pipe", "inherit"], ...options });
}

try {
    const keyringData = run("gpg", ["--batch", "--dearmor"], { input: publicKey + "\n" });
    writeFileSync(keyring, keyringData);
    const records = run("gpg", ["--batch", "--show-keys", "--with-colons", keyring])
        .toString().split("\n").map((line) => line.split(":"));
    if (records.filter(([type]) => type === "pub").length !== 1 ||
        records.find(([type]) => type === "fpr")?.[9] !== fingerprint) {
        throw new Error("The release keyring does not match the configured fingerprint.");
    }
    run("gpg", ["--batch", "--import"], { input: signingKey + "\n" });
    const sign = ["--batch", "--local-user", fingerprint, "--digest-algo", "SHA256"];

    mkdirSync("signed", { recursive: true });
    mkdirSync(pool, { recursive: true });
    mkdirSync(join(apt, "dists/stable/main/binary-amd64"), { recursive: true });
    for (const { platform, suffix } of assetGroups) {
        const directory = join("packages", `thermal-berry-${platform}`);
        const files = assets(directory, suffix);
        if (files.length === 0) throw new Error(`Missing ${suffix} assets in ${directory}.`);
        for (const name of files) copyFileSync(join(directory, name), join("signed", name));
    }

    const marker = 'readonly EMBEDDED_RELEASE_KEYRING=""';
    const installer = readFileSync("install.sh", "utf8").split("\n");
    if (!installer.includes(marker)) throw new Error("Missing release keyring placeholder in install.sh.");
    const embeddedKeyring = `readonly EMBEDDED_RELEASE_KEYRING="${keyringData.toString("base64")}"`;
    writeFileSync("signed/install.sh", installer.map((line) => line === marker ? embeddedKeyring : line).join("\n"));

    const linuxPackages = "packages/thermal-berry-linux";
    const debs = assets(linuxPackages, ".deb");
    if (debs.length !== 1) throw new Error("Expected exactly one Debian package.");
    const deb = join(linuxPackages, debs[0]);
    for (const [field, expected] of [["Package", "thermal-berry"], ["Version", packageVersion], ["Architecture", "amd64"]]) {
        if (run("dpkg-deb", ["-f", deb, field]).toString().replace(/\n+$/, "") !== expected) {
            throw new Error(`Unexpected ${field} in ${deb}; expected ${expected}.`);
        }
    }
    copyFileSync(deb, join(pool, debs[0]));

    // Match the C-locale glob ordering used by the GitHub release verification.
    const names = assetGroups.flatMap(({ suffix }) => assets("signed", suffix));
    const sums = run("sha512sum", ["--", ...names, "install.sh"], { cwd: "signed" });
    writeFileSync("signed/SHA512SUMS", Buffer.concat([
        Buffer.from(`# Product: thermal-berry\n# Version: ${version}\n`),
        sums,
    ]));
    run("gpg", [...sign, "--detach-sign", "SHA512SUMS"], { cwd: "signed" });
    run("gpgv", ["--keyring", keyring, "SHA512SUMS.sig", "SHA512SUMS"], { cwd: "signed" });
    process.stdout.write(run("sha512sum", ["-c", "SHA512SUMS"], { cwd: "signed" }));

    copyFileSync("signed/install.sh", "pages/install.sh");
    copyFileSync(keyring, join(apt, "thermal-berry.gpg"));
    writeFileSync(join(apt, index), run("apt-ftparchive", ["packages", "pool"], { cwd: apt }));
    writeFileSync(join(apt, index + ".gz"), run("gzip", ["-n", "-c", index], { cwd: apt }));
    const validUntil = run("date", ["-Ru", "-d", "+365 days"]).toString();
    const aptMetadata = run("apt-ftparchive", [
        "-o", "APT::FTPArchive::Release::Origin=Thermal Berry",
        "-o", "APT::FTPArchive::Release::Label=Thermal Berry",
        "-o", "APT::FTPArchive::Release::Suite=stable",
        "-o", "APT::FTPArchive::Release::Codename=stable",
        "-o", "APT::FTPArchive::Release::Architectures=amd64",
        "-o", "APT::FTPArchive::Release::Components=main",
        "release", "dists/stable",
    ], { cwd: apt });
    writeFileSync(join(apt, "Release.new"), Buffer.concat([Buffer.from(`Valid-Until: ${validUntil}`), aptMetadata]));
    renameSync(join(apt, "Release.new"), join(apt, release));
    run("gpg", [...sign, "--clearsign", "--output", "dists/stable/InRelease", release], { cwd: apt });
    run("gpgv", ["--keyring", join(apt, "thermal-berry.gpg"), "dists/stable/InRelease"], { cwd: apt });
} finally {
    spawnSync("gpgconf", ["--kill", "gpg-agent"], { env, stdio: "inherit" });
    rmSync(gpgHome, { recursive: true, force: true });
}
