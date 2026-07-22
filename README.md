# Thermal Berry

Desktop app for monitoring and controlling fans/temperature on Alienware
laptops on Linux, with an architecture designed for third parties to add
other vendors.

- **Stack:** Tauri v2 · Rust · React + TypeScript

## Contributing

Contributions are welcome!

1. Branch off `dev`.
2. Open a PR targeting `dev`.
3. Once reviewed and merged, changes land on `master` when the branch is stable enough for release.

## Install

```bash
curl -fsSL https://ramirozein.me/thermal-berry/install.sh | bash
```

Downloads the latest release from [GitHub Releases](https://github.com/ramirozein/thermal-berry/releases),
installing the `.deb` package on apt/dpkg systems or a portable AppImage otherwise.

## Development

```bash
bun install
bun run tauri dev
```

Core tests (curves, conversions, database):

```bash
cd src-tauri && cargo test
cargo test -- --ignored
```

## Architecture

![Thermal Berry Architecture](https://cdn.weberry.site/images/open-source/thermal-berry/architecture.png)
