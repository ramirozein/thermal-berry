# Contributing

Contributions are welcome, including bug reports, feature proposals, documentation, tests, and code.

Thermal Berry aims to have zero telemetry. Please keep this goal in mind when proposing or implementing changes.

Before opening an issue, check for existing reports. Include relevant details such as reproduction steps, expected and actual behavior, operating system, and device model. For substantial changes, open an issue to discuss the approach first.

To contribute code:

1. Create a new branch from the existing `dev` branch.
2. Install dependencies with `bun install` and run the app with `bun run tauri dev`.
3. Run the relevant checks, including `bun run build` and `cargo test --manifest-path src-tauri/Cargo.toml` when applicable.
4. Open a pull request targeting `dev`. Describe your changes, link any related issue, and report the checks performed. Note any checks you could not run.

After review and integration into `dev`, changes reach `master` when the branch is stable enough for release. Please communicate respectfully and provide constructive feedback.

The `.agents` directory and `skills-lock.json` are excluded from the repository so each contributor can use their own custom skills locally.
