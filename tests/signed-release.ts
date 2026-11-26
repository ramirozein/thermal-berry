import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const fixture = mkdtempSync(join(tmpdir(), "thermal-berry-release-test-"));
const gpgHome = join(fixture, "keys");
const env: NodeJS.ProcessEnv = { ...process.env, GNUPGHOME: gpgHome, LC_ALL: "C" };
delete env.THERMAL_BERRY_RELEASE_SIGNING_KEY;
const keyring = join(fixture, "release-keyring.gpg");
const version = "v1.2.0";

process.once("SIGINT", () => process.exit(130));
process.once("SIGTERM", () => process.exit(143));

function run(command: string, args: string[], childEnv: NodeJS.ProcessEnv = env, cwd = fixture) {
    return execFileSync(command, args, { cwd, env: childEnv, stdio: ["ignore", "pipe", "inherit"] });
}

try {
    mkdirSync(gpgHome, { mode: 0o700 });
    run("gpg", [
        "--batch", "--passphrase", "", "--quick-generate-key",
        "Thermal Berry fixture <fixture@example.invalid>", "ed25519", "sign", "1d",
    ]);
    const keys = run("gpg", ["--batch", "--with-colons", "--list-keys"]).toString();
    const fingerprint = keys.split("\n").find((line) => line.startsWith("fpr:"))?.split(":")[9];
    assert.ok(fingerprint, "GPG did not return the test key fingerprint.");

    for (const directory of ["demo/DEBIAN", "packages/thermal-berry-linux", "packages/thermal-berry-windows", "packages/thermal-berry-macos"]) {
        mkdirSync(join(fixture, directory), { recursive: true });
    }
    writeFileSync(join(fixture, "demo/DEBIAN/control"), [
        "Package: thermal-berry", `Version: ${version.slice(1)}`, "Architecture: amd64",
        "Maintainer: Fixture <fixture@example.invalid>", "Description: Synthetic test package", "",
    ].join("\n"));
    run("dpkg-deb", [
        "--build", "--root-owner-group", "demo", `packages/thermal-berry-linux/thermal-berry_${version.slice(1)}_amd64.deb`,
    ]);
    writeFileSync(join(fixture, "packages/thermal-berry-linux/fixture.AppImage"), "replacement\n");
    for (const name of [
        "linux/Z-order.AppImage", "linux/fixture.rpm", "windows/fixture-setup.exe", "macos/fixture.dmg",
    ]) {
        writeFileSync(join(fixture, `packages/thermal-berry-${name}`), "");
    }
    copyFileSync(join(root, "install.sh"), join(fixture, "install.sh"));
    run(process.execPath, [join(root, "scripts/sign-release.ts"), version], {
        ...env,
        THERMAL_BERRY_RELEASE_FINGERPRINT: fingerprint,
        THERMAL_BERRY_RELEASE_SIGNING_KEY: run("gpg", ["--batch", "--armor", "--export-secret-keys", fingerprint]).toString(),
        THERMAL_BERRY_RELEASE_PUBLIC_KEY: run("gpg", ["--batch", "--armor", "--export", fingerprint]).toString(),
    });

    const manifest = run("bash", ["-e", "-c", `
        printf '# Product: thermal-berry\n# Version: %s\n' "$1"
        sha512sum -- *.AppImage *.rpm *-setup.exe *.dmg install.sh
    `, "verify-manifest", version], env, join(fixture, "signed"));
    assert.deepEqual(readFileSync(join(fixture, "signed/SHA512SUMS")), manifest);

    const aptRelease = join(fixture, "pages/apt/dists/stable");
    assert.match(readFileSync(join(aptRelease, "Release"), "utf8"), /^Origin: Thermal Berry$/m);
    assert.match(readFileSync(join(aptRelease, "main/binary-amd64/Packages"), "utf8"), /^Version: 1\.2\.0$/m);

    // Keep APT configuration, lock files, lists, and caches in the fixture.
    const apt = join(fixture, "apt");
    for (const directory of ["empty", "lists", "archives"]) mkdirSync(join(apt, directory), { recursive: true });
    writeFileSync(join(apt, "status"), "");
    writeFileSync(join(apt, "sources.list"), `deb [arch=amd64 signed-by=${keyring}] file:${fixture}/pages/apt stable main\n`);
    const aptConfig = join(apt, "config");
    writeFileSync(aptConfig, `Dir::Etc::main "/dev/null";
Dir::Etc::parts "${apt}/empty";
Dir::Etc::sourcelist "${apt}/sources.list";
Dir::Etc::sourceparts "${apt}/empty";
Dir::State::status "${apt}/status";
Dir::State::lists "${apt}/lists";
Dir::Cache::archives "${apt}/archives";
Dir::Cache::pkgcache "";
Dir::Cache::srcpkgcache "";
`);
    const aptEnv = { ...env, APT_CONFIG: aptConfig };
    process.stdout.write(run("apt-get", ["update"], aptEnv));
    assert.match(run("apt-cache", ["policy", "thermal-berry"], aptEnv).toString(), /Candidate: 1\.2\.0/);
    for (const file of readdirSync(join(apt, "lists"))) {
        if (file.endsWith("_InRelease")) rmSync(join(apt, "lists", file));
    }
    rmSync(join(aptRelease, "InRelease"));
    const unsigned = spawnSync("apt-get", ["update"], { cwd: fixture, env: aptEnv, encoding: "utf8" });
    if (unsigned.error) throw unsigned.error;
    assert.ok(unsigned.status, "APT accepted a repository without its signature.");
    assert.match(unsigned.stdout + unsigned.stderr, /not signed/i);

    const mockBin = join(fixture, "mock-bin");
    mkdirSync(mockBin);
    writeFileSync(join(mockBin, "curl"), `#!/usr/bin/env bun
import { copyFileSync } from "node:fs";
import { join } from "node:path";

const args = process.argv.slice(2);
const url = args.find((arg) => arg.startsWith("https://")) ?? "";
const name = url.slice(url.lastIndexOf("/") + 1);
if (url.startsWith("https://api.github.com/")) {
    const tag = name === "latest" ? ${JSON.stringify(version)} : name;
    const base = "https://github.com/ramirozein/thermal-berry/releases/download/" + tag;
    console.log(JSON.stringify({
        tag_name: tag,
        assets: ["fixture.AppImage", "SHA512SUMS", "SHA512SUMS.sig"].map(
            (asset) => ({ browser_download_url: base + "/" + asset }),
        ),
    }));
} else if (url.startsWith("https://github.com/") && args.includes("-o")) {
    copyFileSync(join(process.env.BERRY_FIXTURE, "signed", name), args[args.indexOf("-o") + 1]);
} else {
    process.exitCode = 1;
}
`, { mode: 0o755 });
    const installerEnv = {
        ...env,
        // Use the published installer's embedded keyring, even if overrides are set locally.
        THERMAL_BERRY_RELEASE_KEYRING: "",
        THERMAL_BERRY_RELEASE_PUBLIC_KEY: "",
        BERRY_FIXTURE: fixture,
        PATH: [mockBin, process.env.PATH].filter(Boolean).join(delimiter),
    };
    const installed = join(fixture, "installed");
    writeFileSync(installed, "original\n");

    function installerArgs(tag: string) {
        return ["pages/install.sh", "--update-appimage", installed, tag];
    }

    function reject(tag: string, error: string) {
        const result = spawnSync("bash", installerArgs(tag), { cwd: fixture, env: installerEnv, encoding: "utf8" });
        if (result.error) throw result.error;
        assert.ok(result.status, "Installer accepted an invalid update.");
        assert.ok((result.stdout + result.stderr).includes(error), result.stdout + result.stderr);
        assert.equal(readFileSync(installed, "utf8"), "replacement\n");
    }

    run("bash", ["install.sh", "--update-appimage", installed, version], {
        ...installerEnv,
        THERMAL_BERRY_RELEASE_KEYRING: keyring,
    });
    assert.equal(readFileSync(installed, "utf8"), "replacement\n");
    writeFileSync(installed, "original\n");
    process.stdout.write(run("bash", installerArgs(version), installerEnv));
    assert.deepEqual(readFileSync(installed), readFileSync(join(fixture, "signed/fixture.AppImage")));
    reject("v1.3.0", "does not match the requested version");
    writeFileSync(join(fixture, "signed/fixture.AppImage"), "modified\n");
    reject(version, "checksum did not match");
    writeFileSync(join(fixture, "signed/SHA512SUMS.sig"), "invalid\n");
    reject(version, "signature is invalid");
    rmSync(join(fixture, "signed/SHA512SUMS.sig"));
    reject(version, "Could not download release signature");
} finally {
    spawnSync("gpgconf", ["--kill", "gpg-agent"], { env, stdio: "inherit" });
    rmSync(fixture, { recursive: true, force: true });
}
