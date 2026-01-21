# RECCH

<div align="center">
  <img src="src-tauri/icons/icon.png" width="128" height="128" alt="Recch Icon" />
  <h3>Modern Database Management Tool</h3>
  <p>Cross-platform, secure, and intelligent database manager built with Rust & Vue.</p>

  [![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

  **[English](#features) | [中文](#功能特性)**
</div>

---

## Features

- **🚀 Multi-Database Support**: Seamlessly connect to MySQL, PostgreSQL, and Redis.
- **🤖 Smart Assistant**: Natural language to SQL/Redis commands conversion for efficient querying.
- **🎨 Modern UI**: Clean, responsive interface with Dark/Light themes powered by Naive UI.
- **🛠️ Structure Designer**: Visual table schema editor for managing columns, keys, and indexes.
- **🔒 Secure & Local**: All connection data is stored locally. No cloud sync required.
- **🖥️ Cross-Platform**: Native performance on macOS, Windows, and Linux via Tauri.

## Tech Stack

- **Frontend**: Vue 3, TypeScript, Vite, Naive UI
- **Backend**: Rust, Tauri, SQLx, Redis
- **Architecture**: Local-first, secure, and high-performance.

## Development

### Prerequisites

- Node.js (v16+)
- Rust (Stable)

### Setup

```bash
# Install frontend dependencies
npm install

# Run backend/frontend in development mode
npm run tauri dev
```

### Build

```bash
# Build for production
npm run tauri build
```

## License

MIT License.

---

<div align="center">
  <h1>RECCH</h1>
  <img src="src-tauri/icons/icon.png" width="128" height="128" alt="Recch Icon" />
  <h3>现代化数据库管理工具</h3>
  <p>基于 Rust 和 Vue 构建的跨平台、安全、智能的数据库管理器</p>
</div>

## 功能特性

- **🚀 多数据库支持**：无缝连接 MySQL、PostgreSQL 和 Redis。
- **🤖 智能助手**：自然语言转 SQL/Redis 命令，高效查询。
- **🎨 现代化界面**：基于 Naive UI 的简洁响应式界面，支持深色/浅色主题。
- **🛠️ 结构设计器**：可视化表结构编辑器，轻松管理列、主键和索引。
- **🔒 安全本地化**：所有连接数据存储在本地，无需云同步。
- **🖥️ 跨平台**：通过 Tauri 在 macOS、Windows 和 Linux 上实现原生性能。

## 技术栈

- **前端**：Vue 3、TypeScript、Vite、Naive UI
- **后端**：Rust、Tauri、SQLx、Redis
- **架构**：本地优先、安全、高性能

## 开发指南

### 环境要求

- Node.js (v16+)
- Rust (Stable)

### 安装运行

```bash
# 安装前端依赖
npm install

# 开发模式运行
npm run tauri dev
```

### 构建

```bash
# 构建生产版本
npm run tauri build
```

## 许可证

MIT 许可证
