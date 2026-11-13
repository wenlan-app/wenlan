# 设置与数据：安装、模型、备份与离线队列

本指南涵盖安装与更新、可选的模型支持后台工作、数据位置与安全保留，以及 CLI 离线 outbox。

语言：[English](setup-and-data.md) · [繁體中文](setup-and-data.zh-Hant.md) · [简体中文](setup-and-data.zh-Hans.md) · [Español](setup-and-data.es-ES.md)<br>
[返回简体中文 README](../README.zh-Hans.md)

目录：[安装](#installation) · [模型与隐私](#models) · [备份与卸载](#backup-and-removal) · [离线队列](#offline-queue)

<a id="installation"></a>

## 安装

### 下载并打开 Wenlan

[下载桌面版](https://github.com/wenlan-app/wenlan/releases/latest)，安装后打开：

- **macOS（Apple Silicon）：** 打开 `.dmg`，把 Wenlan 拖进「应用程序」。App 已签名并通过公证。
- **Windows x64：** 运行 `-setup.exe`。安装包尚未签名。如果 SmartScreen 显示警告，请先确认文件来自官方 Releases 页面，再点「更多信息」→「仍要运行」。
- **Linux x64（v0.22 起）：** 下载 `.AppImage`，设为可执行（`chmod +x Wenlan_*.AppImage`）后打开；或在 Debian、Ubuntu 上安装 `.deb`（`sudo apt install ./Wenlan_*_amd64.deb`）。需要 Ubuntu 24.04、Debian 13、Fedora 40，或其他 glibc 2.39 以上且带 WebKitGTK 4.1 的发行版。安装包没有系统厂商签名；之后会和其他平台一样自动更新。ARM64 Linux 暂时没有桌面版；可按照[设置指南](setup-with-ai.md#install-the-runtime)，不通过桌面 app，直接搭配 AI 工具使用。

首次启动会下载本地搜索模型，设置完成前请保持联网。[下载与隐私说明](PRIVACY.md#when-wenlan-reaches-the-network)。

### 其他安装方式与更新

后台整理与自动更新是可选功能，需[另外配置模型](#models)；这与首次启动下载的搜索模型不同。

**从终端安装 macOS app**

安装程序会下载 app、核对 SHA-256，并放进「应用程序」：

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/wenlan-app/wenlan/main/scripts/install-macos-app.sh)"
```

**不使用桌面 app**

在 macOS Apple Silicon 上运行：

```bash
npx -y wenlan setup
```

`npx` 需要 Node.js；若未安装，可先运行 `curl -fsSL https://raw.githubusercontent.com/wenlan-app/wenlan/main/install.sh | bash`，再运行 `wenlan setup --basic`。

这个命令会下载预编译的 CLI、后台服务（daemon）与 MCP 连接器，启动并验证本地服务；不需要安装 Rust 或 Cargo。使用 glibc 的 Linux x64/ARM64 可以采用自动化的 [shell 设置流程](setup-with-ai.md#install-the-runtime)；Windows x64 请从 [Releases](https://github.com/wenlan-app/wenlan/releases/latest) 下载对应的 archive。macOS Intel 目前[没有受支持的完整 runtime 安装方式](../crates/wenlan-cli/README.md#macos-intel)。

**安装内容与更新方式**

桌面 app 内置 daemon、CLI 与 MCP 连接器，打开时会启动 daemon，并提供检测到的 AI 工具接入选项：Claude Code、Codex 使用 plugin，其他支持工具使用 MCP 设置。不使用桌面 app 时，运行的也是同一个 daemon；两种方式都让 AI 工具访问同一个本地知识库。

更新 macOS app 时，把新 app 拖到旧 app 上覆盖并打开。Wenlan 0.17.0 及更早的版本需要先手动退出。

手动与各工具设置说明：[AI 辅助设置](setup-with-ai.md) · [Claude Code plugin](../plugin/README.md) · [Codex plugin](../plugin-codex/README.md) · [CLI 与 MCP](../crates/wenlan-cli/README.md)。

<a id="models"></a>

## 模型与隐私

先用已连接的 AI 写页面，不必另装本地语言模型。后台整理与自动更新是可选功能，需要另外设置模型。

- **搜索在本机运行。** 首次启动会下载搜索模型，之后在你的电脑上运行，不需要 API key。
- **连接 AI，就可能传出内容。** 云端 AI 可将读取的知识传给其提供商；选用云端模型整理资料，也会发送该任务所需的内容。本地存储不代表这些交互都留在本机。
- **使用统计默认关闭。** 同意后才发送有限的操作计数、版本与平台，不包含知识内容或安装标识。[查看说明](PRIVACY.md#telemetry)。

模型下载、更新检查、远程图片与可选远程访问的联网行为，请见[隐私说明](PRIVACY.md#when-wenlan-reaches-the-network)。

### 模型选项与技术细节

- **本地基础检索：** [BGE 向量模型](https://huggingface.co/Qdrant/bge-base-en-v1.5-onnx-Q) 通过 FastEmbed 在你的设备上运行，用于混合搜索，不需要 API key。
- **可选的设备端整理：** 内容补充与页面汇总可使用你选择的 [`Qwen3.5 4B`](https://huggingface.co/unsloth/Qwen3.5-4B-GGUF) 或 [`Qwen3.5 9B`](https://huggingface.co/unsloth/Qwen3.5-9B-GGUF)，通过 [llama.cpp](https://github.com/ggml-org/llama.cpp) 运行。未经选择，不会下载或启用语言模型。
- **其他模型来源：** Ollama 或 LM Studio 等 OpenAI 兼容的本地端点，或已设置的云端 provider，也可以提供模型支持的内容补充与页面汇总。
- **在 Mac 上用 MLX：** 想用苹果 MLX 格式的模型，可以在 LM Studio 或 Ollama 里加载它，再到 Wenlan 设置里选这个模型来源。内容补充、页面汇总和逐句核对来源都会用它。
- **云端说明：** 如果你选择的模型端点位于远端，Wenlan 会把该任务需要的 system prompt 与 user prompt 发给它。本地检索与设备端整理仍留在你的设备上。

完整 workflow 参考：[plugin/skills](../plugin/skills/README.md)。模型分工与限制见：[技术基础（英文）](technical-foundations.md#model-roles)。

<a id="backup-and-removal"></a>

## 备份与卸载

页面与会话笔记是 Markdown 文件；记忆与图谱保存在本地数据库。卸载 app 时，可以保留你的知识数据。

### 默认位置

- 页面与会话笔记：`~/.wenlan/`。
- 数据库与运行环境数据：macOS 为 `~/Library/Application Support/wenlan/`，Linux 为 `~/.local/share/wenlan/`，Windows 为 `%LOCALAPPDATA%\wenlan\`。
- 以文件保存的数据：数据库文件夹里的 `files-mirror/`。每个改动都会先写进这里的写入记录（`files-mirror/log/`），再写进数据库；`files-mirror/files/` 里可读的文件会在几分钟内跟上。数据库只是这些文件的索引：Wenlan 启动时如果数据库文件不见了或是空的，会用这些文件和写入记录重建数据库，连最后的改动也在，然后在后台重新检查页面和出处。检查沿用 AI 之前的判断，没改过的内容不用再判断一次。Wenlan 每天也会检查一次这些文件能重建出同样的数据库。请不要删除这个文件夹。设置 `WENLAN_FILES_MIRROR=0` 可以关闭，那时就像以前的版本一样只写数据库。

### 备份知识数据

复制文件夹前，先退出 app 并停止后台服务。自定义页面或数据位置也要备份。只导出 wiki 不等于完整的数据库备份。

如果曾从 Origin 升级，请另外检查 `~/.origin/` 与同级的 `origin` 平台数据文件夹是否留有旧数据。备份旧数据时一并保留；只有打算删除时才移除。

### 卸载程序

在设置中关闭「登录时在后台运行Wenlan 」，退出 app，再删除 `Wenlan.app` 或运行 Windows 卸载程序。知识数据文件夹会保留；只有确定不再需要数据，并已完成所需备份时，才另外删除。

`wenlan background off` 会停止 daemon 并关闭自动启动，但不会移除服务注册。仅使用 CLI 的卸载方式、AI 工具中残留的设置、凭据与其他文件，请见[完整卸载说明](PRIVACY.md#data-deletion)。

<a id="offline-queue"></a>

## 离线队列（outbox）

如果本地守护进程无法访问，`wenlan capture` 和 `wenlan brief update` 会把请求写入本地持久化队列（outbox）并正常退出。守护进程恢复后，它会通过常规 HTTP 路由排空这些写入；用 `wenlan outbox status` 查看队列，或用 `wenlan outbox drain` 立即重放。被守护进程直接拒绝的写入（4xx，例如未通过内容质量检查）会带着回执移动到 `outbox/failed/`，而不是无限重试；传输失败或服务器错误（5xx）则留在队列中等待下一次排空，排空每 60 秒自动运行一次。
