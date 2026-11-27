# Graver

[English](README.md) | [简体中文](README.zh-CN.md) | [繁體中文](README.zh-TW.md) | **日本語**

[![CI](https://github.com/Skymly/graver/actions/workflows/check.yml/badge.svg)](https://github.com/Skymly/graver/actions/workflows/check.yml)

Windows IME の骨格実装です。システムエントリーポイントと変換エンジンは Rust、設定画面は F# と Avalonia で実装されています。

現時点では OS への登録は行われず、他アプリケーションでの文字入力もできません。テキストサービスオブジェクト自体は生成可能で、有効化されると名前付きパイプ接続を維持してキー入力を変換サービスへ渡します。登録完了までメモ帳には読み込まれません。

## 構成

| パス | 役割 |
| --- | --- |
| `crates/graver-engine` | 変換セッション管理（Windows 非依存） |
| `crates/graver-ipc` | 長さプレフィックス付きフレーミングと JSON プロトコル |
| `crates/graver-service` | ユーザーセッション内の常駐プロセス（Windows サービスではない） |
| `crates/graver-tsf` | ホストが読み込む TSF DLL（未登録） |
| `Graver.UI` | 設定ウィンドウ |

テキストサービスの CLSID は `crates/graver-tsf` に固定されています。登録完了前は変更しないでください。パイプ名は `\\.\pipe\Graver` です。

## ビルド

Rust 1.98 と .NET 10 が必要です。

```powershell
cargo test --workspace
dotnet build Graver.slnx
```

## 動作確認

先にサービスを起動し、その後設定ウィンドウを開いて「Probe Service（探测服务）」をクリックします。

```powershell
cargo run -p graver-service
dotnet run --project Graver.UI
```

各パイプ接続は独立した変換セッションを持ちます。現行の `latin-buffer` スキームは文字をバッファリングし、Space または Enter で確定します。`Ctrl` や `Alt` を押しながらのキー入力はホストへ返却されます。パイプは現在のユーザーのみに許可され、リモートクライアントは拒否されます。

## 未実装事項

- テキストサービスは未登録です。`regsvr32` は失敗し、IME 一覧に Graver は表示されず、メモ帳での入力もまだできません。
- エンジンには `pinyin-min` がありますが、テキストサービスは依然として `latin-buffer` を使用しており、設定ウィンドウも新スキームへ接続されていません（中国語入力は未対応）。
- 五筆配列および辞書ファイルはまだありません。
- 32 ビット版 DLL はまだありません。システム登録時は 32 ビット版と 64 ビット版の DLL で同一の CLSID を共有する必要があります。
- 候補ウィンドウ、インストーラー、スキームエディターは未実装です。候補情報はプロトコル上にのみ存在し、この段階では表示されません。

## 禁止事項

`graver-tsf` に変換エンジン、Serde、Avalonia をリンクしないでください。この DLL はメモ帳・ブラウザ・ゲームなどへ読み込まれ、ホスト側の AppContainer 制限を受けます。DLL 内で非同期ランタイムを起動しないでください。AppContainer から接続させるためにパイプの ACL を緩和することも禁止です。接続できない場合はキー入力をそのままホストへ返してください。
