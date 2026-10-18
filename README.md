<div align="center">

<img src="./website/logo.png" alt="WorkMami" width="88" />

# WorkMami

**WorkBuddy 的妈咪，稳稳接住你 🐾**

多账号管理 · 本地反代 · 批量签到 · 猫猫旅行托管 · Token 看板

macOS（Apple 芯片 / Intel）&nbsp;·&nbsp; Windows

[![License: MIT](https://img.shields.io/badge/License-MIT-pink.svg)](./LICENSE)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey)
![Version](https://img.shields.io/badge/version-v0.1.0-ec4899)

</div>

---

> ⚠️ **免责声明**：WorkMami 是一款**第三方独立开发**的本地效率工具，与 WorkBuddy 官方**无任何隶属或背书关系**，亦未获其授权。本项目仅供个人学习与技术研究使用，请在遵守 WorkBuddy 官方服务条款的前提下合理使用，**一切非法用途责任自负**。

## 它是什么

WorkMami 把散落在多个 WorkBuddy 账号里的额度、任务和签到，收进一个顺手的本地管理台。它不改动、不侵入 WorkBuddy 官方服务，只是在你自己的电脑上帮你把账号、模型、任务和用量管得明明白白，并能把模型能力以标准接口复用到其它客户端。

## 核心功能

| 模块 | 说明 |
| --- | --- |
| **账号管理** | 通过 OAuth 网页授权添加多个 WorkBuddy 账号，凭证本机加密存储，按昵称区分、一键切换 |
| **本地反代** | 把 WorkBuddy 模型以 OpenAI 兼容接口在本机反代出来，供任意支持自定义模型的客户端接入；真流式透传、智能选号、粘性会话、模型回落，长任务稳定不断流 |
| **签到中心** | 一键批量签到，自动检测当日是否已签、防重复；可开启后台定时自动签到 |
| **猫猫旅行托管** | 每天自动派猫猫出行、到站领奖，展示"派出于 / 奖励领于"时刻；后台每 60 分钟巡检，连续确认全部账号达当日上限后自动熔断，降低调用频率防风控 |
| **Token 看板** | 各账号余额折线图、反代调用流水与用量统计、积分清单双口径归因 |
| **数据管理** | 会话跨账号迁移、整库备份与导出，数据全程留在本地 |

## 下载

前往 [Releases](../../releases) 页面，选择与你设备匹配的版本：

| 文件 | 平台 | 系统要求 |
| --- | --- | --- |
| `WorkMami_0.1.0_aarch64.dmg` | macOS · Apple 芯片（M1/M2/M3/M4） | macOS 12+ |
| `WorkMami_0.1.0_x64.dmg` | macOS · Intel 芯片 | macOS 12+ |
| `WorkMami_0.1.0_win_x64.exe` | Windows · 绿色免安装单文件 | Windows 10 / 11（64 位） |

> 不确定 Mac 芯片？点左上角  → "关于本机"：显示 **M 开头**选 Apple 芯片版，显示 **Intel** 选 Intel 版。

## 安装注意事项

### macOS

1. 双击 `.dmg`，把 **WorkMami** 拖入"应用程序"。
2. 由于未做付费签名与公证，首次打开若提示 **"已损坏，应移到废纸篓"**，这是未签名 App 的正常现象，在终端执行以下命令清除隔离标记即可：
   ```bash
   xattr -cr /Applications/WorkMami.app
   # 若提示权限不足：
   sudo xattr -rd com.apple.quarantine /Applications/WorkMami.app
   ```
3. 若只是"身份不明的开发者"提示，也可**右键 App → 打开 → 再点"打开"**绕过。

### Windows

1. 绿色单文件，**双击即可运行**，无需安装、不写注册表。
2. 未做代码签名，首次运行可能被 SmartScreen 拦截，点 **"更多信息" → "仍要运行"**；也可用 PowerShell 解除：
   ```powershell
   Unblock-File -Path .\WorkMami_0.1.0_win_x64.exe
   ```
3. 需要 **WebView2 运行库**（Win10/11 一般自带）；若打开后白屏，请安装 Microsoft Edge WebView2 Evergreen Runtime。

## 隐私与数据

- 完全本地运行，**不上传任何数据**。
- 账号凭证以 **AES-GCM 加密**保存在本机数据目录，并配合系统钥匙串（macOS Keychain / Windows 凭据管理器）。
- 数据目录：macOS `~/Library/Application Support/WorkMami`，Windows `%APPDATA%\WorkMami`。
- 分发出去的安装包是一个**空白的全新程序**，绝不包含任何人的账号数据。

## 开发与构建

**环境要求**：Node.js 18+、Rust（stable，rust-version 1.80+）、macOS 需 Xcode Command Line Tools、Windows 需 MSVC 工具链与 WebView2。

```bash
# 安装依赖
npm install

# 开发模式（热更新）
npm run tauri dev

# 构建正式包（macOS 会产出 .app 与 .dmg）
npm run tauri build
```

在 Apple 芯片上为 Intel Mac 交叉构建：

```bash
rustup target add x86_64-apple-darwin
npx tauri build --target x86_64-apple-darwin
```

在 macOS 上交叉编译 Windows 绿色版（依赖 [cargo-xwin](https://github.com/rust-cross/cargo-xwin)）：

```bash
rustup target add x86_64-pc-windows-msvc
# 注意：必须带 --features custom-protocol，否则 release 不会内嵌前端资源，
# 运行时会去连 devUrl(127.0.0.1) 导致首开白屏/报错
cargo xwin build --release --features custom-protocol --target x86_64-pc-windows-msvc
# 产物：src-tauri/target/x86_64-pc-windows-msvc/release/workmami.exe
```

> Windows 的 NSIS / MSI 安装包只能在真实 Windows 环境用 `npm run tauri build` 生成，无法在 macOS 上交叉产出，因此跨平台分发以绿色 exe 为准。

## 技术栈

- **桌面框架**：Tauri 2（Rust 后端 + 系统 WebView）
- **前端**：React 18 · TypeScript · Vite · Ant Design · ECharts · Zustand · dayjs
- **后端能力**：axum（本地反代服务）· reqwest（流式透传）· tokio-cron-scheduler（定时巡检/签到）· rusqlite（用量存储）· aes-gcm / keyring（凭证加密）

## 项目结构

```
.
├── src/                # React 前端（页面在 src/pages）
├── src-tauri/          # Rust 后端（api / proxy / scheduler / state / vault 等）
├── website/            # 工具官网（index.html 单页）
├── scripts/            # 辅助脚本
└── LICENSE             # MIT
```

## 许可证

本项目基于 [MIT License](./LICENSE) 开源。

## 联系

- 个人博客：www.majiabin.com
- 交流可提 Issue，或关注公众号：

<div align="center">
  <img src="./website/wechat-qr.jpg" alt="老马公众号二维码" width="160" />
  <br/>
  <sub>扫一扫关注公众号</sub>
</div>

---

<div align="center">

**如果 WorkMami 帮到了你，欢迎点个 ⭐ Star。**

</div>
