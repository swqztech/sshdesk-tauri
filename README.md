# 连枢（SSHDesk）— Tauri 版

SSH 主机台：多主机、终端、文件管理、概览、Mongo / Qdrant 备份（Rust 后端 + React UI）。

## 开发

```bash
npm install
npm run dev
```

## 打包

```bash
npm run build:client
```

产物：`src-tauri/target/release/sshdesk-tauri.exe` 与 `src-tauri/target/release/bundle/nsis/*.exe`。

## 数据目录

默认 `~/.sshdesk`（与 Electron 版共用）。
