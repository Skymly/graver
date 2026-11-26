# graver

Windows IME skeleton combining a Rust TSF/engine/IPC workspace (`Cargo.toml`, Rust 1.98) with an F# + Avalonia settings UI (`Graver.slnx`, .NET 10).

## Build & Verify

```powershell
# Matches .github/workflows/check.yml on windows-latest:
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
dotnet build Graver.slnx
```

- Local smoke test: run `cargo run -p graver-service` first, then `dotnet run --project Graver.UI`.
- Requires Rust `1.98` (see `rust-toolchain.toml`) and .NET `10` (`global.json`).

## Architecture & Invariants

- **Crate boundaries**: `crates/graver-engine` stays Windows-independent; `crates/graver-ipc` owns length-prefixed JSON framing over `\\.\pipe\Graver`; `crates/graver-service` runs as a per-user process granting pipe access only to the current user and rejecting remote clients.
- **Fixed CLSID**: The TSF text-service CLSID in `crates/graver-tsf` must **not** be changed before system registration is finalized (future 32-bit and 64-bit DLLs must share the same CLSID).
- **Input handling**: Keys held with `Ctrl` or `Alt` must pass back to the host application; `unsafe_op_in_unsafe_fn` is warned workspace-wide.

## Conventions

- Commit messages, issues, and PR text: **English**.
- Do not commit or push unless explicitly requested.
