# Development

## Prerequisites

- Rust (stable, supports edition 2024) with the WebAssembly target:
  ```bash
  rustup target add wasm32-unknown-unknown
  ```
- Trunk, which builds and serves the Leptos frontend:
  ```bash
  cargo install trunk
  ```
- Tauri CLI:
  ```bash
  cargo install tauri-cli --version "^2"
  ```
- Windows: Microsoft C++ Build Tools and WebView2 (preinstalled on Windows 11).
  See the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for other platforms.

## Run in development

```bash
cargo tauri dev
```

This runs `trunk serve` on `http://localhost:1420`, then starts the desktop window. Frontend
changes in `src/` reload automatically; backend changes in `src-tauri/` trigger a rebuild and
restart.

## Build a release

```bash
cargo tauri build
```

Produces `target/release/bundle/msi/*.msi` and `target/release/bundle/nsis/*-setup.exe`.
Always try the release build after changing the CSP in `tauri.conf.json`: dev mode does not
enforce it.

Runs `trunk build` into `dist/`, then builds and bundles the app (installers end up under
`target/release/bundle/`).

## Project layout

```
AeonDataStudio/
├── Cargo.toml          # frontend crate + workspace root
├── Trunk.toml          # frontend build/serve settings
├── index.html          # Trunk entry point
├── styles.css
├── public/             # static assets copied into dist/
├── shared/             # types shared by frontend and backend
├── src/                # Leptos frontend (WASM)
├── src-tauri/          # Tauri backend
│   ├── src/lib.rs      # app setup and command registration
│   ├── capabilities/   # permissions granted to the window
│   └── tauri.conf.json
└── docs/               # this documentation
```

## Conventions

**Adding a backend command**
1. Write the function in `src-tauri/src/commands.rs` (split into a `commands/` folder once it
   grows), marked `#[tauri::command]`. Return `AppResult<T>` rather than panicking.
2. Register it in `tauri::generate_handler![...]` in `src-tauri/src/lib.rs`.
3. Add a typed wrapper in `src/api.rs` so components never call `invoke` with raw strings.
   Argument structs use `#[serde(rename_all = "camelCase")]`: Tauri expects `jobId` for a
   Rust parameter named `job_id`.
4. Put any new request/response types in `shared/src/lib.rs`.

**Calling the backend from the frontend.** `src/api.rs` declares the `invoke` binding with
`catch`, so a failed command becomes an `Err` instead of a panic:

```rust
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], catch)]
    async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}
```

**Long-running work** goes through the job runner (see [architecture.md](architecture.md)) and
reports progress with events. Never block a command handler on a full dataset pass.

**Tests.** Pipeline steps are plain Rust and should have unit tests with small inline
documents covering keep, drop, and edge cases (empty text, non-UTF-8, very long lines).

```bash
cargo test -p aeondatastudio
```

**Checking the UI in a normal browser.** `trunk serve` alone (port 1420) shows only "Could not
start", because `window.__TAURI__` exists only inside the Tauri window. Use `cargo tauri dev`
for real testing.
