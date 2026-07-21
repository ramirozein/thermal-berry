# Thermal Berry

Desktop app for monitoring and controlling fans/temperature on Alienware
laptops on Linux, with an architecture designed for third parties to add
other vendors.

- **Stack:** Tauri v2 · Rust · React + TypeScript

## Development

```bash
bun install
bun run tauri dev
```

Core tests (curves, conversions, database):

```bash
cd src-tauri && cargo test
# with real Alienware hardware:
cargo test -- --ignored
```

## Architecture

![Thermal Berry Architecture](https://cdn.weberry.site/images/open-source/thermal-berry/architecture.png)
