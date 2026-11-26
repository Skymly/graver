# Graver

[English](README.md) | [简体中文](README.zh-CN.md) | **繁體中文** | [日本語](README.ja.md)

Windows 輸入法骨架。系統進入點與組字引擎使用 Rust，設定介面使用 F# 與 Avalonia。

目前尚不會註冊進系統中，也無法在其他程式內輸入文字。文字服務物件已可建立；啟用後會維持一條具名管道連線，將按鍵轉交給組字服務。在完成註冊前，記事本不會載入它。

## 目錄配置

| 路徑 | 角色 |
| --- | --- |
| `crates/graver-engine` | 組字工作階段，不依賴 Windows |
| `crates/graver-ipc` | 長度前綴訊框與 JSON 通訊協定 |
| `crates/graver-service` | 使用者工作階段內的常駐行程，非 Windows 服務 |
| `crates/graver-tsf` | 供宿主載入的 TSF DLL，目前尚未註冊 |
| `Graver.UI` | 設定視窗 |

文字服務的 CLSID 固定於 `crates/graver-tsf`。完成註冊前請勿修改。管道名稱為 `\\.\pipe\Graver`。

## 建置

需要 Rust 1.98 與 .NET 10。

```powershell
cargo test --workspace
dotnet build Graver.slnx
```

## 試執行

先啟動服務，再開啟設定視窗並點擊「探測服務」。

```powershell
cargo run -p graver-service
dotnet run --project Graver.UI
```

每個管道連線擁有各自的組字工作階段。目前方案 `latin-buffer` 僅緩衝字元，按空白鍵或 Enter 送出上屏。按住 Ctrl 或 Alt 的按鍵會交還宿主處理。管道僅授權給目前使用者，並拒絕遠端用戶端。

## 尚未實作

- 文字服務尚未註冊。`regsvr32` 仍會失敗，輸入法清單不會出現 Graver，記事本亦無法輸入。
- 引擎已實作 `pinyin-min`，但文字服務仍使用 `latin-buffer`，設定視窗尚未切換接線至新方案，因此記事本還不能輸入中文。
- 尚未支援五筆，亦無詞庫檔案。
- 尚無 32 位元 DLL。註冊至系統時，32 位元與 64 位元 DLL 必須使用相同的 CLSID。
- 尚無候選字視窗、安裝程式與方案編輯器。候選項目目前僅存在於通訊協定中，此階段不顯示。

## 禁止事項

請勿將組字引擎、Serde 或 Avalonia 連結進 `graver-tsf`。該 DLL 會被載入記事本、瀏覽器與遊戲等行程，並需承受宿主的 AppContainer 限制。請勿在 DLL 內啟動 async runtime，也勿為了讓 AppContainer 連上管道而放寬 ACL；無法連線時直接將按鍵交還宿主。
