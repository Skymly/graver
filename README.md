# Graver

**English** | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | [日本語](README.ja.md)

[![CI](https://github.com/Skymly/graver/actions/workflows/check.yml/badge.svg)](https://github.com/Skymly/graver/actions/workflows/check.yml)

Windows IME skeleton. The system entrypoint and composition engine are written in Rust, while the settings UI uses F# and Avalonia.

It does not register into the OS or accept text input in other applications yet. The text service object can be instantiated; once activated, it maintains a pipe connection and forwards keystrokes to the composition service. Until registered, Notepad will not load it.

## Layout

| Path | Role |
| --- | --- |
| `crates/graver-engine` | Composition sessions, Windows-independent |
| `crates/graver-ipc` | Length-prefixed framing and JSON protocol |
| `crates/graver-service` | Per-user resident session process (not a Windows service) |
| `crates/graver-tsf` | TSF DLL loaded by host applications (not yet registered) |
| `Graver.UI` | Settings window |

The text service CLSID is fixed in `crates/graver-tsf`. Do not change it before registration is complete. The named pipe is `\\.\pipe\Graver`.

## Build

Requires Rust 1.98 and .NET 10.

```powershell
cargo test --workspace
dotnet build Graver.slnx
```

## Smoke Run

Start the service first, then launch the settings window and click **Probe Service** (探测服务).

```powershell
cargo run -p graver-service
dotnet run --project Graver.UI
```

Each pipe connection maintains its own composition session. The current `latin-buffer` scheme only buffers characters and commits on Space or Enter. Keystrokes held with `Ctrl` or `Alt` are passed back to the host. The pipe grants access only to the current user and rejects remote clients.

## Not Implemented Yet

- Text service is not registered. `regsvr32` still fails, Graver does not appear in the input method list, and Notepad cannot type with it yet.
- The engine has `pinyin-min`, but the text service still uses `latin-buffer` and the settings window does not switch schemes yet (Chinese input in Notepad is not ready).
- No Wubi scheme or dictionary files yet.
- No 32-bit DLL yet. When registering with Windows, 32-bit and 64-bit DLLs must share the exact same CLSID.
- No candidate window, installer, or scheme editor yet. Candidates remain in the protocol only and are not rendered at this stage.

## Do Not

Do not link the composition engine, Serde, or Avalonia into `graver-tsf`. That DLL will be loaded into Notepad, browsers, and games under the host's AppContainer restrictions. Do not spin up an async runtime inside the DLL. Do not weaken the pipe ACL to let AppContainer connect; pass keys back to the host when the pipe is unreachable.
