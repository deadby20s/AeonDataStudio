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

## Continuous integration and releases

- `.github/workflows/ci.yml` runs the tests, clippy (warnings are errors) and a WebAssembly
  build on Linux and Windows for every push to `main` and every pull request.
- `.github/workflows/release.yml` builds installers for Windows (`.msi`, `.exe`), macOS
  (`.dmg`, Apple silicon and Intel) and Linux (`.deb`, `.rpm`, `.AppImage`).

To publish a release:

1. Bump `version` in `src-tauri/tauri.conf.json` (and the crates' `Cargo.toml`), and add a
   section to `CHANGELOG.md`.
2. Push a tag: `git tag -a v0.1.1-alpha -m "..." && git push origin v0.1.1-alpha`.
3. The workflow uploads all installers to a **draft** pre-release for that tag. Check it on
   GitHub, then publish it.

Pull requests that change `release.yml`, and manual runs (Actions → Release → Run workflow),
only build and keep the installers as workflow artifacts for 14 days.
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
├── core/               # aeon-core: the engine (no UI); ops.rs, api.rs, mcp.rs, plugins.rs, llm.rs
├── cli/                # aeon-cli: the `aeon` CLI as a library (CLI, `serve`, `mcp`)
├── src/                # Leptos frontend (WASM)
├── src-tauri/          # Tauri app: commands.rs wraps aeon-core; src/bin/aeon.rs is the CLI binary
│   ├── src/lib.rs      # app setup and command registration
│   ├── capabilities/   # permissions granted to the window
│   └── tauri.conf.json
├── examples/plugins/   # example plugins
└── docs/               # this documentation
```

## Conventions

**Adding an operation**
1. Write it as a method on `AppState` in `core/src/ops.rs`. Return `AppResult<T>`; anything
   slow starts a job. The CLI, API and MCP server can then reach it.
2. Expose it where it's needed:
   - the HTTP API: a route in `core/src/api.rs` `handle` (and a line in docs/api.md);
   - MCP: a tool in `core/src/mcp.rs` that calls that route;
   - the app: a thin `#[tauri::command]` in `src-tauri/src/commands.rs`, registered in
     `tauri::generate_handler![...]` in `src-tauri/src/lib.rs`.
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

## Auto-updates

Installed apps check `https://github.com/deadby20s/AeonDataStudio/releases/download/updater/latest.json`
on start (Settings → Updates to check by hand). Updates are signed, so they need a key pair
that only the maintainer holds. One-time setup:

1. Generate the key pair (choose a password when asked; keep the `.key` file private and
   backed up — losing it means installed apps can no longer be updated):
   ```bash
   cargo tauri signer generate -w ~/.tauri/aeondatastudio.key
   ```
2. Add two repository secrets on GitHub (Settings → Secrets and variables → Actions):
   `TAURI_SIGNING_PRIVATE_KEY` = the contents of `~/.tauri/aeondatastudio.key`, and
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = the password. With the GitHub CLI:
   ```bash
   gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.tauri/aeondatastudio.key
   ```
   ```bash
   gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD
   ```
3. Put the **public** key (contents of `~/.tauri/aeondatastudio.key.pub`) into
   `src-tauri/updater-pubkey.txt` and commit it.

From then on, tagged release builds sign their installers and upload `latest.json`;
publishing the release runs `.github/workflows/updater-channel.yml`, which copies
`latest.json` to the `updater` release that apps check. Versions built before step 3 can't
update themselves; users install the first signed version by hand.
