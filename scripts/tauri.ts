import {chmodSync, mkdtempSync, rmSync, statSync} from "node:fs";
import {tmpdir} from "node:os";
import {join} from "node:path";

const args = process.argv.slice(2);
const cli = require.resolve("@tauri-apps/cli/tauri.js");

let scratch: string | undefined;
if (args[0] === "dev" && process.platform !== "win32") {
    const parent = tmpdir();
    const {mode, uid} = statSync(parent);
    if ((uid !== 0 && uid !== process.getuid?.()) || ((mode & 0o022) !== 0 && (mode & 0o1000) === 0)) {
        throw new Error("The development temp directory must be private or have the sticky bit set.");
    }
    scratch = mkdtempSync(join(parent, "thermal-berry-dev-"));
}

try {
    if (scratch) chmodSync(scratch, 0o700);
    const child = Bun.spawn([process.execPath, cli, ...args], {
        env: scratch ? {...process.env, TMPDIR: scratch} : process.env,
        stdin: "inherit",
        stdout: "inherit",
        stderr: "inherit",
    });
    const interrupt = () => child.kill("SIGINT");
    const terminate = () => child.kill("SIGTERM");
    process.on("SIGINT", interrupt);
    process.on("SIGTERM", terminate);
    try {
        process.exitCode = await child.exited;
    } finally {
        process.off("SIGINT", interrupt);
        process.off("SIGTERM", terminate);
    }
} finally {
    if (scratch) rmSync(scratch, {recursive: true, force: true});
}
