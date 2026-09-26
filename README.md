# Graver

Windows 输入法骨架。系统入口和组字引擎用 Rust，设置界面用 F# 和 Avalonia。

现在不会注册到系统里，也不能在其他程序中输入。文本服务对象可以创建；激活后它保持一条管道连接，把按键交给组字服务。没有注册之前，记事本不会加载它。

## 布局

| 路径 | 角色 |
| --- | --- |
| `crates/graver-engine` | 组字会话，不依赖 Windows |
| `crates/graver-ipc` | 长度前缀帧和 JSON 协议 |
| `crates/graver-service` | 用户会话里的常驻进程，不是 Windows 服务 |
| `crates/graver-tsf` | 供宿主加载的 TSF DLL，目前未注册 |
| `Graver.UI` | 设置窗口 |

文本服务的 CLSID 固定在 `crates/graver-tsf`。注册完成之前不要改它。管道名是 `\\.\pipe\Graver`。

## 构建

需要 Rust 1.98 和 .NET 10。

```powershell
cargo test --workspace
dotnet build Graver.slnx
```

## 试运行

先启动服务，再打开设置窗口，点「探测服务」。

```powershell
cargo run -p graver-service
dotnet run --project Graver.UI
```

每个管道连接有自己的组字会话。当前方案 `latin-buffer` 只缓冲字符，空格或回车上屏。按住 Ctrl 或 Alt 的键会交还宿主。管道只授予当前用户，并拒绝远程客户端。

## 还没做

- 文本服务还没有注册。`regsvr32` 会失败，输入法列表里不会出现 Graver，也不能在记事本中输入。
- 没有拼音或五笔，也没有词库。
- 没有 32 位 DLL。注册进系统时，32 位和 64 位 DLL 必须使用同一个 CLSID。
- 没有候选窗、安装器和方案编辑器。候选仍留在协议里，这一阶段不显示。

## 不要做的事

不要把引擎、Serde 或 Avalonia 链进 `graver-tsf`。这个 DLL 会被加载进记事本、浏览器和游戏，还要承受宿主的 AppContainer 限制。不要在 DLL 里启动 async runtime。不要为了让 AppContainer 连上管道而放宽 ACL；连不上时交还按键。
