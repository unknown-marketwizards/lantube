# LanTube frontend

For a standalone executable, run `cargo build --release` from the repository root. Cargo builds and embeds the UI.

For frontend development, run `cargo run` from the repository root, then run these commands in `frontend`.

```sh
npm ci
npm run dev
```

Vite proxies `/api` and `/file` to `127.0.0.1:8888`. Run `npm run build` to type-check and build the UI separately.
