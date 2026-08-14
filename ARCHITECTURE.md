# NeeCoder 项目架构文档

> NeeCoder 是一款基于 **Tauri 2.0 + React + Rust** 构建的 AI 编程助手桌面应用，集成了代码编辑器、AI 对话面板、智能代码补全、RAG 代码搜索、多 Agent 协作、MCP 协议桥接、沙箱安全、Skill 系统、终端 PTY 和 LSP 语言服务器等功能。

---

## 目录

- [NeeCoder 项目架构文档](#neecoder-项目架构文档)
  - [目录](#目录)
  - [1. 总体架构](#1-总体架构)
  - [2. 技术栈](#2-技术栈)
  - [3. 目录结构](#3-目录结构)
  - [4. 后端架构 (Rust)](#4-后端架构-rust)
    - [4.1 应用入口与生命周期 (lib.rs)](#41-应用入口与生命周期-librs)
    - [4.2 LLM 通信层 (llm/mod.rs)](#42-llm-通信层-llmmodrs)
    - [4.3 Agent 系统 (agent/mod.rs)](#43-agent-系统-agentmodrs)
    - [4.4 Agent Hook 系统 (agent/hooks.rs)](#44-agent-hook-系统-agenthooksrs)
    - [4.5 循环检测器 (agent/loop\_detector.rs)](#45-循环检测器-agentloop_detectorrs)
    - [4.6 上下文压缩 (agent/context.rs)](#46-上下文压缩-agentcontextrs)
    - [4.7 Checkpoint 机制 (agent/checkpoint.rs)](#47-checkpoint-机制-agentcheckpointrs)
    - [4.8 Cloud Agent (agent/cloud.rs)](#48-cloud-agent-agentcloudrs)
    - [4.9 Agent 工具集 (agent/tools/mod.rs)](#49-agent-工具集-agenttoolsmodrs)
    - [4.10 子 Agent 调度 (agent/sub\_agent.rs)](#410-子-agent-调度-agentsub_agentrs)
    - [4.11 Agent 定义与注册 (agent/definition.rs)](#411-agent-定义与注册-agentdefinitionrs)
    - [4.12 对话管理 (chat/mod.rs)](#412-对话管理-chatmodrs)
    - [4.13 记忆系统 (memory/mod.rs)](#413-记忆系统-memorymodrs)
    - [4.14 代码补全 (completion/mod.rs)](#414-代码补全-completionmodrs)
    - [4.15 RAG 代码索引与搜索 (rag/mod.rs)](#415-rag-代码索引与搜索-ragmodrs)
    - [4.16 LSP 语言服务器 (lsp/mod.rs)](#416-lsp-语言服务器-lspmodrs)
    - [4.17 MCP 协议客户端 (mcp/mod.rs)](#417-mcp-协议客户端-mcpmodrs)
    - [4.18 沙箱安全 (sandbox/mod.rs)](#418-沙箱安全-sandboxmodrs)
    - [4.19 Skill 系统 (skill/mod.rs)](#419-skill-系统-skillmodrs)
    - [4.20 遥测系统 (telemetry/mod.rs)](#420-遥测系统-telemetrymodrs)
    - [4.21 文件系统监听 (fs\_watcher/mod.rs)](#421-文件系统监听-fs_watchermodrs)
    - [4.22 配置管理 (config/mod.rs)](#422-配置管理-configmodrs)
    - [4.23 日志系统 (logging/mod.rs)](#423-日志系统-loggingmodrs)
    - [4.24 命令层 (Tauri Commands)](#424-命令层-tauri-commands)
  - [5. 前端架构 (React + TypeScript)](#5-前端架构-react--typescript)
    - [5.1 入口与根组件](#51-入口与根组件)
    - [5.2 核心组件](#52-核心组件)
    - [5.3 Tauri API 抽象层](#53-tauri-api-抽象层)
    - [5.4 主题与样式系统](#54-主题与样式系统)
  - [6. 前后端通信协议](#6-前后端通信协议)
    - [invoke 命令（请求/响应）](#invoke-命令请求响应)
    - [Tauri Events（推送）](#tauri-events推送)
  - [7. 数据流与关键路径](#7-数据流与关键路径)
    - [Agent 消息完整流程](#agent-消息完整流程)
  - [8. 安全机制](#8-安全机制)
    - [多层安全防护](#多层安全防护)
  - [9. 测试覆盖](#9-测试覆盖)
  - [10. 依赖清单](#10-依赖清单)
    - [Rust 后端](#rust-后端)
    - [前端](#前端)

---

## 1. 总体架构

```
┌──────────────────────────────────────────────────────────────┐
│                      Tauri 2.0 桌面应用                        │
├──────────────────────────┬───────────────────────────────────┤
│    前端 (React + TS)       │         后端 (Rust)                │
│  ┌──────────────────────┐ │  ┌──────────────────────────────┐ │
│  │ App.tsx (根组件)      │ │  │ Commands (Tauri invoke)      │ │
│  ├──────────────────────┤ │  ├──────────────────────────────┤ │
│  │ ChatPanel            │ │  │ Agent 系统 (41 工具 + Hook)  │ │
│  │ CodeEditor           │◄┼─►│ LLM 通信层 (多 Provider)     │ │
│  │ FileExplorer         │ │  │ RAG 代码索引 (BM25+向量)     │ │
│  │ TerminalPanel (xterm)│ │  │ 记忆系统 (会话/长期/艾宾浩斯) │ │
│  │ InlineEdit           │ │  │ MCP 协议客户端               │ │
│  │ SearchPanel/Settings │ │  │ 沙箱安全 / Skill 系统        │ │
│  │ CloudAgentPanel      │ │  │ 遥测 / LSP / 文件监听        │ │
│  └──────────────────────┘ │  └──────────────────────────────┘ │
│  useTauri.ts (API 层)    │  Tauri State + Invoke Handler      │
├──────────────────────────┤  Tauri Event System                │
└──────────────────────────┴───────────────────────────────────┘
```

**核心通信方式：**
- **invoke (请求/响应)**：前端调用后端命令（如 `send_message`, `read_file`）
- **Tauri Events (推送)**：后端向前端推送流式事件（如 `chat-event`, `completion-event`, `pty-output`）
- **Tauri State (共享状态)**：后端通过 `app.manage()` 注册全局状态，命令通过 `State<'_, T>` 访问

---

## 2. 技术栈

| 层级 | 技术 | 用途 |
|------|------|------|
| 桌面框架 | Tauri 2.0 | 跨平台桌面应用框架，Rust 后端 + WebView 前端 |
| 后端语言 | Rust (Edition 2024) | 核心逻辑、文件操作、LLM 通信 |
| 前端框架 | React 19 + TypeScript | UI 组件、状态管理 |
| 构建工具 | Vite | 前端打包与开发服务器 |
| 编辑器 | CodeMirror 6 | 代码编辑、语法高亮、幽灵文本补全 |
| 终端 | xterm.js + portable-pty | 真实 PTY 终端模拟 |
| 异步运行时 | Tokio | Rust 异步任务、流式处理 |
| HTTP 客户端 | reqwest | LLM API 调用（流式 SSE） |
| 数据库 | SQLite (rusqlite) | RAG 代码索引持久化存储 |
| Token 计数 | tiktoken-rs | 精确 token 计数 (o200k_base) |
| 文件监听 | notify | 文件系统变更检测与自动重索引 |
| 序列化 | serde + serde_json + serde_yaml | JSON/YAML 序列化 |
| 日志 | log + 自定义 DualLogger | 双输出日志（控制台 + 文件轮转） |

---

## 3. 目录结构

```
NeeCoder/
├── src/                              # 前端源码
│   ├── main.tsx                      # React 入口
│   ├── App.tsx                       # 根组件（布局、文件标签页管理）
│   ├── components/
│   │   ├── ChatPanel.tsx             # AI 对话面板（最复杂组件）
│   │   ├── CodeEditor.tsx            # CodeMirror 6 编辑器封装
│   │   ├── FileExplorer.tsx          # 文件树浏览器
│   │   ├── TerminalPanel.tsx         # xterm.js 终端面板（PTY 前端）
│   │   ├── InlineEdit.tsx            # 内联编辑（LLM 驱动的代码修改）
│   │   ├── SearchPanel.tsx           # 代码搜索面板
│   │   ├── Settings.tsx              # 设置界面 + MCP 管理
│   │   ├── StatusBar.tsx             # 底部状态栏
│   │   ├── CloudAgentPanel.tsx       # 云 Agent 后台任务管理
│   │   ├── ContextMenu.tsx           # 右键菜单
│   │   ├── MentionMenu.tsx           # @ 文件提及菜单
│   │   ├── Overlay.tsx               # 全局遮罩层（Agent 提问/确认）
│   │   └── SyntaxHighlighterWrapper.tsx
│   ├── hooks/useTauri.ts             # Tauri API 统一抽象层
│   └── styles/global.css             # Catppuccin Mocha 暗色主题
├── src-tauri/                        # 后端源码
│   ├── src/
│   │   ├── main.rs / lib.rs          # 入口 / Tauri 应用构建器
│   │   ├── agent/                    # Agent 系统
│   │   │   ├── mod.rs                # Agent 主循环
│   │   │   ├── hooks.rs              # Lifecycle Hook 框架（7 个 Hook）
│   │   │   ├── loop_detector.rs      # 循环检测（4 策略 + 2 级裁决）
│   │   │   ├── context.rs            # 上下文压缩（token 预算制）
│   │   │   ├── checkpoint.rs         # Git Checkpoint 机制
│   │   │   ├── cloud.rs              # Cloud Agent 后台执行
│   │   │   ├── token_count.rs        # tiktoken 精确 token 计数
│   │   │   ├── definition.rs         # Agent 定义与注册表
│   │   │   ├── sub_agent.rs          # 子 Agent 调度
│   │   │   ├── tools/                # 41 个 Agent 工具
│   │   │   └── utils.rs              # 辅助函数
│   │   ├── chat/mod.rs               # 对话消息模型与事件
│   │   ├── commands/                  # Tauri 命令（11 个模块）
│   │   │   ├── agent.rs / chat.rs / cloud.rs / completion.rs
│   │   │   ├── config.rs / edit_inline.rs / lsp.rs / mcp.rs
│   │   │   ├── project.rs / pty.rs / search.rs / skill.rs
│   │   │   ├── workspace.rs / review.rs / dependency_graph.rs
│   │   ├── completion/               # 代码补全
│   │   │   ├── mod.rs                # FIM 补全核心
│   │   │   └── multi_file.rs         # 多文件上下文采集
│   │   ├── config/mod.rs             # 配置管理（XOR 加密 API Key）
│   │   ├── fs_watcher/mod.rs         # 文件系统监听
│   │   ├── llm/mod.rs                # LLM API 通信（多 Provider）
│   │   ├── logging/mod.rs            # 双输出日志系统
│   │   ├── lsp/mod.rs                # LSP 协议实现
│   │   ├── mcp/                      # MCP 协议客户端
│   │   │   ├── mod.rs                # JSON-RPC 2.0 类型定义
│   │   │   ├── client.rs             # MCP 客户端（stdio 传输）
│   │   │   └── tool_bridge.rs        # MCP 工具桥接到 Agent
│   │   ├── memory/                   # 记忆系统
│   │   │   ├── mod.rs                # MemoryManager 统一接口
│   │   │   ├── session_store.rs      # Markdown 会话存储
│   │   │   ├── long_term.rs          # 长期记忆（MEMORY.md）
│   │   │   ├── ebbinghaus.rs         # 艾宾浩斯遗忘曲线记忆
│   │   │   ├── preferences.rs        # 用户偏好追踪
│   │   │   ├── agent_log.rs          # Agent JSONL 审计日志
│   │   │   ├── notes.rs              # 每日笔记
│   │   │   ├── search.rs             # 记忆搜索
│   │   │   └── tools.rs              # 记忆工具（供 Agent 使用）
│   │   ├── rag/mod.rs                # RAG 代码索引（混合搜索）
│   │   ├── sandbox/mod.rs            # 沙箱安全系统
│   │   ├── skill/                    # Skill 系统
│   │   │   ├── mod.rs                # SkillManager + 模板引擎
│   │   │   └── builtin.rs            # 内置 Skill 定义
│   │   └── telemetry/mod.rs          # 遥测/使用分析系统
│   ├── tools.json                    # Agent 工具 JSON Schema
│   └── Cargo.toml                    # Rust 依赖清单
```

---

## 4. 后端架构 (Rust)

### 4.1 应用入口与生命周期 ([lib.rs](src-tauri/src/lib.rs))

`lib.rs` 是整个后端的核心入口，负责：

1. **初始化日志系统** — 调用 `logging::init()` 设置双输出日志
2. **注册 Tauri 插件** — shell、dialog、fs、process、clipboard
3. **初始化全局 State** — 通过 `app.manage()` 注册以下共享状态：
   - `ConfigState` — 配置管理器 (`Arc<RwLock<ConfigManager>>`)
   - `AppSettings` — 当前设置 (`Arc<RwLock<AppSettings>>`)
   - `ChatState` — 对话记忆 (`Arc<RwLock<ConversationMemory>>`)
   - `CompletionCache` — 补全 LRU 缓存（最大 200 条）
   - `CancelMap` / `AgentCancelMap` — 补全/Agent 取消标志
   - `CheckpointStore` — Git checkpoint 存储
   - `FileSnapshots` / `FileSnapshotStore` — 编辑快照与文件 undo
   - `LspManager` — LSP 管理器
   - `CodeIndexer` — RAG 代码索引器（启动时从 SQLite 加载）
   - `QuestionAwaiters` / `ConfirmAwaiters` — Agent 交互等待通道
   - `ToolRegistry` — 工具定义注册表（从 `tools.json` 加载）
   - `AgentRegistry` — Agent 定义注册表（从 `agents.json` 加载）
   - `McpRegistry` + MCP 工具定义 — MCP 服务器注册表
   - `CloudTaskState` — 云 Agent 任务管理器
   - `PtyState` — 终端 PTY 状态
   - `TelemetryCollector` — 遥测数据收集器
   - `SkillState` — Skill 管理器
   - `FileWatcher` — 文件系统监听器
4. **启动后台任务**：
   - MCP 服务器自动连接（启动后 1 秒）
   - 文件变更自动重索引（10 秒轮询）
5. **注册 invoke_handler** — 暴露 50+ 个 Tauri 命令

**关键设计决策：** 所有全局状态使用 `Arc<RwLock<T>>` 或 `Arc<Mutex<T>>` 包装。`RwLock`（Tokio）用于读多写少且临界区跨 `.await` 的场景，`std::sync::Mutex` 用于简单互斥且临界区不跨 `.await` 的场景。

### 4.2 LLM 通信层 ([llm/mod.rs](src-tauri/src/llm/mod.rs))

提供统一的 LLM API 调用接口，支持 **4 种 Provider**：

| Provider | 用途 | 默认 Base URL |
|----------|------|---------------|
| OpenAI | 通用 | `api.openai.com/v1` |
| DeepSeek | 默认 Provider | `api.deepseek.com/v1` |
| Anthropic | Claude 系列 | `api.anthropic.com/v1` |
| Ollama | 本地部署 | `localhost:11434` |

**两种调用模式：**
- **`stream_fim`** — Fill-in-the-Middle 补全模式
- **`stream_chat`** — 对话模式，支持工具调用（function calling）

**核心功能：**
- SSE 流式读取 + `on_token` 回调
- `sanitize_messages` — 消息格式净化（确保 tool/tool_calls 配对正确）
- `CancelFlag` (`Arc<AtomicBool>`) 支持取消流式请求

### 4.3 Agent 系统 ([agent/mod.rs](src-tauri/src/agent/mod.rs))

Agent 是 NeeCoder 的核心 AI 执行引擎，采用**迭代循环**模式：

```
用户消息 → LLM 推理 → 工具调用 → Hook 链处理 → 工具结果反馈 → LLM 再推理 → ... → 完成
```

**主循环流程 (`run_agent` → `run_no_dispatch`)：**
1. 将用户消息追加到对话历史
2. 调用 `stream_chat` 向 LLM 发送请求
3. 解析 LLM 响应中的 `tool_calls`
4. 对每个工具调用：
   - **Pre-tool Hook 链** — SnapshotHook 快照、ConfirmHook 确认等
   - 通过 `ToolExecutor` 执行工具（2 分钟超时保护）
   - **Post-tool Hook 链** — OutputTruncateHook 截断、AutoDiagnoseHook 诊断等
   - 检查 `PostExecuteAction`（更新 Todo、提问用户、调度子 Agent）
5. **Post-tool-batch Hook 链** — AuditLogHook 审计等
6. **循环检测** — LoopDetector 检查是否有非进展模式
7. **Checkpoint** — 自动创建 git checkpoint
8. 重复直到 LLM 不再调用工具或达到最大迭代次数

**Agent 可观测性事件（通过 Tauri Events 发射）：**
`AgentThinking` / `AgentStatus` / `ToolCall` / `ToolResult` / `ToolRetry` / `TodoUpdate` / `AskUserQuestion` / `ConfirmRequest` / `ContextTrimmed` / `AgentLog` / `EditDiff`

### 4.4 Agent Hook 系统 ([agent/hooks.rs](src-tauri/src/agent/hooks.rs))

可插拔的有序 Hook 链，替代硬编码逻辑。提供三级钩子：

**`LifecycleHook` trait：**
```rust
#[async_trait]
pub trait LifecycleHook: Send + Sync {
    fn name(&self) -> &str;
    async fn pre_tool(...) -> HookResult;        // Continue | Deny | ModifyArgs
    async fn post_tool(...) -> PostHookResult;    // 修改结果 / 注入消息
    async fn post_tool_batch(...) -> BatchHookResult; // 批次完成后注入消息
}
```

**13 个内置 Hook：**

| Hook | 级别 | 功能 |
|------|------|------|
| `SnapshotHook` | pre_tool | 文件修改前快照（支持 undo/rollback） |
| `ConfirmHook` | pre_tool | 危险操作确认（delete/terminal） |
| `SensitiveDataFilterHook` | post_tool | 过滤敏感信息（API Key 等） |
| `PromptInjectionGuardHook` | post_tool | 标记不可信内容注入模式 |
| `ErrorPatternHook` | post_tool | 检测工具输出中的错误模式与重试循环 |
| `AutoRollbackHook` | post_tool | 重复验证失败时回滚文件修改 |
| `TddGateHook` | post_tool | 根据 run_tests 结果驱动 TDD 状态机相位转换 |
| `FailureMemoryHook` | post_tool | 失败模式写入记忆库供后续参考 |
| `PreviewImageHook` | post_tool | 将 `[SCREENSHOT]` 标记转为 base64 视觉消息注入 LLM |
| `AuditLogHook` | post_tool_batch | 记录工具调用审计日志 |
| `OutputTruncateHook` | post_tool | 超长工具输出安全截断 |
| `FileChangeTrackerHook` | post_tool | 文件变更事件推送到前端 |
| `AutoDiagnoseHook` | post_tool_batch | 文件修改后自动诊断并注入修复提示 |

**执行机制：** `post_tool_batch_chain` 遍历所有 Hook，但仅对覆写了相应方法的 Hook 执行实际逻辑；其余 Hook 因默认空实现不产生副作用。

### 4.5 循环检测器 ([agent/loop_detector.rs](src-tauri/src/agent/loop_detector.rs))

检测四种非进展模式，采用两级裁决状态机：

```
Continue ──(detected)──> InjectWarning ──(detected again)──> HardStop
```

**四种检测策略：**

| 策略 | 描述 | 默认阈值 |
|------|------|---------|
| No-Progress Repeat | 相同 (tool + args + output_hash) 重复 N 次 | 5 |
| Ping-Pong | 两个工具交替 A→B→A→B | 3 周期 |
| Consecutive Failure | 同一工具连续失败 N 次 | 5 |
| Read-Only Streak | 连续只读操作（去重后重复读同一目标） | 15 次 / 3 次重复 |

**读写隔离：** 12 种只读工具（`read_file`、`grep`、`glob` 等）单独追踪，读不同文件视为有效探索。

### 4.6 上下文压缩 ([agent/context.rs](src-tauri/src/agent/context.rs))

基于 token 预算的分级压缩策略：

**参数：**
- `COMPACT_THRESHOLD = 0.80` — 达到 80% token 预算时触发
- `PRESERVE_RECENT = 6` — 始终保留最近 6 条消息
- `MIN_MESSAGES_FOR_COMPACT = 8` — 至少 8 条消息才考虑压缩

**压缩流程：**
1. 使用 tiktoken 精确计算总 token 数
2. 若未超阈值 → 返回不变
3. 保留：首条用户消息 + 最近 6 条消息
4. **Tool-call 安全边界**：确保不拆分 `assistant(tool_calls) → tool` 消息对
5. 中间部分 → LLM 摘要 → 注入为压缩消息
6. 压缩前执行 **Pre-compaction Flush**：提取最多 8 条持久事实注入长期记忆

### 4.7 Checkpoint 机制 ([agent/checkpoint.rs](src-tauri/src/agent/checkpoint.rs))

基于 Git 的项目状态快照：

- 每次 Agent 迭代修改文件后自动创建 checkpoint
- 通过 `git add` + `git commit` 实现（commit message: `checkpoint: iteration N - description`）
- `CheckpointStore` 按 session_id 分组管理
- 支持回滚到任意 checkpoint（`git checkout`）

### 4.8 Cloud Agent ([agent/cloud.rs](src-tauri/src/agent/cloud.rs))

后台异步 Agent 执行系统：

**`CloudTask` 结构：**
- `id` / `session_id` / `status` (Pending/Running/Completed/Failed/Cancelled)
- `message` — 任务描述
- `pr_config` — 可选的自动 PR 配置

**PR 自动创建：** 完成后可自动执行 `git push` + 创建 GitHub Pull Request

**`CloudTaskManager`：** 管理所有云任务的生命周期，支持并发执行

### 4.9 Agent 工具集 ([agent/tools/mod.rs](src-tauri/src/agent/tools/mod.rs))

每个工具实现 `Tool` trait：

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String;
    fn post_execute_action(&self, args: &serde_json::Value) -> PostExecuteAction { ... }
}
```

**`ToolContext`** — 工具运行时共享环境：
- `project_path` / `indexer` / `sandbox` / `app_handle` / `session_id`
- `tavily_api_key` / `llm_provider` / `llm_api_key` / `llm_model`

**`ToolExecutor`** — 带超时保护的工具调度器：
- `execute()` — 2 分钟超时 (`DEFAULT_TOOL_TIMEOUT_MS = 120_000`)
- `register_raw()` — 支持动态注册 MCP 桥接工具

**41 个内置工具一览：**

| 分类 | 工具 | 功能 | 确认 |
|------|------|------|:---:|
| **文件** | `read_file` | 读取文件内容 | |
| | `write_file` | 写入/创建文件 | |
| | `append_file` | 追加内容到文件末尾 | |
| | `delete_file` | 删除文件 | **是** |
| | `edit` | 精确字符串替换（首选编辑方式） | |
| **目录** | `list_directory` | 列出目录内容 | |
| | `create_directory` | 创建目录（含父目录） | |
| | `delete_directory` | 递归删除目录 | **是** |
| **搜索** | `search_codebase` | RAG 语义代码搜索 | |
| | `grep` | 文本模式搜索 | |
| | `glob` | glob 模式文件查找 | |
| | `get_symbols` | 提取文件中的符号定义 | |
| | `get_diagnostics` | 获取编译器/linter 诊断 | |
| | `memory_search` | 记忆系统语义搜索 | |
| **终端/构建** | `run_terminal_command` | 执行 shell 命令（一次性进程） | **是** |
| | `run_terminal_session` | 持久化 PTY shell 会话（cd/环境变量跨调用保留） | |
| | `run_tests` | 识别项目类型（Cargo/npm/pytest/go）并运行测试，只读 | |
| | `run_build` | 识别项目类型并运行构建，返回退出码 + 错误定位 | |
| **Git** | `git_status` | 查看 Git 仓库状态 | |
| | `git_diff` | 查看文件差异 | |
| | `git_commit` | 提交变更（`auto_summary` 参数让 LLM 从 staged diff 推导提交消息，失败回退 name-status 摘要） | |
| | `git_log` | 查看提交历史 | |
| | `git_blame` | 查看文件逐行修改历史 | |
| | `git_branch` | 管理分支 | |
| | `git_push` | 推送到远程 | |
| | `git_checkout` | 切换分支/恢复文件 | |
| | `git_stash` | 暂存/恢复工作区 | |
| **网络/浏览器** | `web_search` | Tavily 网页搜索（降级 DuckDuckGo） | |
| | `web_fetch` | 获取网页内容（HTML 转纯文本） | |
| | `web_preview` | 无头 Edge/Chrome 截取 Web 应用截图，`[SCREENSHOT]` 标记经 PreviewImageHook 注入视觉消息 | |
| | `web_browser` | 持久化 CDP 浏览器自动化：navigate/click/type/screenshot/get_text/close，页面跨调用保持存活 | |
| **测试/质量** | `generate_tests` | LLM 按语言约定生成测试（tests/、*.test.ts、test_*.py、*_test.go）并默认运行，生成-运行-修复闭环 | |
| | `coverage` | 行覆盖率引导：`cargo llvm-cov` 报告未覆盖行（JSON 缓存 + 过滤） | |
| | `tdd` | TDD 状态机启停/查看，相位转换由 TddGateHook 从 run_tests 结果驱动 | |
| | `auto_fix` | 失败命令诊断：LLM 输出根因 + 修复步骤 + 重试命令，闭环"失败→诊断→修复→重试" | |
| **交互** | `ask_user_question` | 向用户提问并阻塞等待回答 | |
| | `todo_write` | 创建/更新任务列表 | |
| | `generate_diagram` | 生成 Mermaid 图表 | |
| **调度** | `dispatch_agent` / `dispatch_agents` | 调度子 Agent（串行/并行 + 文件冲突检测） | |
| | `a2a_invoke` | 调用远程 A2A 协议 Agent | |

### 4.10 子 Agent 调度 ([agent/sub_agent.rs](src-tauri/src/agent/sub_agent.rs))

支持两种调度模式：

- **`run_sub_agent`** — 串行执行单个子 Agent，调用 `run_no_dispatch()`（不支持嵌套调度）
- **`run_sub_agents_parallel`** — 多 Agent 调度，包含**文件冲突检测** + **依赖关系解析**（`depends_on`），然后串行执行

**结果格式：** 成功 `[SUB_AGENT_RESULT:{id}]\n{result}` / 失败 `[SUB_AGENT_ERROR:{id}]\n{error}`

### 4.11 Agent 定义与注册 ([agent/definition.rs](src-tauri/src/agent/definition.rs))

```rust
pub struct AgentDefinition {
    pub id: String, pub name: String, pub description: String,
    pub system_prompt: String, pub tool_names: Vec<String>,
    pub model: Option<String>, pub temperature: Option<f32>,
    pub max_iterations: Option<usize>, pub max_tokens: Option<u32>,
}
```

**4 个内置 Agent：**

| Agent | 角色 | 迭代上限 |
|-------|------|:-:|
| `orchestrator` | 主 Agent / 调度器 | 15 |
| `code_writer` | 代码编写专家 | 10 |
| `debugger` | 调试专家 | 8 |
| `reviewer` | 代码审查（只读） | 5 |

**加载优先级：** `agents.json` 文件 → 内嵌默认定义。支持通过 `save_agent` 命令自定义。

### 4.12 对话管理 ([chat/mod.rs](src-tauri/src/chat/mod.rs))

**消息模型：** `ChatMessage { role, content, tool_calls, tool_call_id, images }`

**对话请求：** `ChatRequest { messages, context, mode }`
- `ChatContext` — `active_file`、`selected_code`、`file_mentions`、`symbol_mentions`、`images`
- `ChatMode` — `Ask` / `Edit` / `Agent`

**事件枚举 `ChatEvent`（21 种变体）：**
`Started` → `Delta` → `Finished` → `ToolCall` → `ToolResult` → `ToolRetry` → `TodoUpdate` → `AskUserQuestion` → `ConfirmRequest` → `AgentThinking` → `AgentStatus` → `AgentLog` → `ContextTrimmed` → `EditDiff` → `Error` / `Cancelled` → `FileRestored` → `CheckpointCreated` → `BudgetExhausted` → `PlanCreated` / `PlanApproved` / `PlanRejected`

**三种模式：**
- **Ask** — 直接 `stream_chat`，流式返回
- **Edit** — 专用 `EDIT_SYSTEM_PROMPT`，返回代码变更建议（带 diff 预览）
- **Agent** — `tokio::spawn` 后台任务 → 完整工具循环 → 持久化 → Dreaming

### 4.13 记忆系统 ([memory/mod.rs](src-tauri/src/memory/mod.rs))

`MemoryManager` 统一接口，包含 **6 个子系统**：

| 子系统 | 文件 | 功能 |
|--------|------|------|
| 会话存储 | `session_store.rs` | Markdown 文件持久化，每会话一个目录 |
| 长期记忆 | `long_term.rs` | `MEMORY.md` 文件，Dreaming 自动追加 |
| 艾宾浩斯记忆 | `ebbinghaus.rs` | 10 种分类（Core/Pattern/Decision/Lesson/BugFix/Api/Perf/Coding/General/Custom），时间衰减 |
| 用户偏好 | `preferences.rs` | 工具使用统计、文件类型分布、任务模式追踪 |
| Agent 审计日志 | `agent_log.rs` | JSONL 格式，记录每个 Agent 会话的完整事件流 |
| 记忆搜索 | `search.rs` | 递归搜索 `.md` 文件，相关性评分 |

**艾宾浩斯遗忘曲线 (`ebbinghaus.rs`)：**
- 10 种 `MemoryCategory`，Core 类不衰减，General 类激进衰减
- 每条记忆含 `strength`（强度）、`last_review`（上次复习时间）、`review_count`（复习次数）
- 支持 Dreaming 自动提取和分类记忆

**记忆注入：** `inject_memory_context()` 在 Agent 启动前注入 MEMORY.md + 近期笔记 + 高权重艾宾浩斯记忆

### 4.14 代码补全 ([completion/mod.rs](src-tauri/src/completion/mod.rs))

采用 **FIM (Fill-in-the-Middle)** 范式：

**上下文采集 (`CompletionContext`)：**
- `prefix` / `suffix` — 光标前后代码
- `imports` — import 语句
- `enclosing_fn` — 当前函数签名
- `related_context` — **多文件上下文**（`multi_file.rs` 采集同目录相关文件的公共符号）

**LRU 缓存：** 基于 `(file_path, prefix_tail, suffix_head)` 哈希键，最多 200 条

**后处理：** 清理 LLM 生成文本 — 去除前导空白行、尾随空白、修正缩进

### 4.15 RAG 代码索引与搜索 ([rag/mod.rs](src-tauri/src/rag/mod.rs))

`CodeIndexer` 实现**混合搜索**：

**代码分块：** 按函数/类/模块结构化分块，支持 30+ 种文件扩展名

**搜索模式：**
- `search()` — 向量余弦相似度
- `bm25_search()` — BM25 关键词（预计算 df_map）
- `hybrid_search()` — 向量 60% + BM25 40% 加权融合

**持久化：** SQLite (`save_to_db` / `load_from_db`)，启动时自动加载

**自动重索引：** 后台循环每 10 秒轮询文件变更，自动重索引

### 4.16 LSP 语言服务器 ([lsp/mod.rs](src-tauri/src/lsp/mod.rs))

`LspManager` 管理 10 种语言服务器（Rust/TS/Python/Go/Java/C/Ruby/PHP/C#/Kotlin）

**实现方法：** `start_lsp` / `did_open` / `did_change` / `did_close` / `get_symbols` / `get_hover` / `shutdown_all`

**通信：** JSON-RPC 2.0 over stdin/stdout

### 4.17 MCP 协议客户端 ([mcp/mod.rs](src-tauri/src/mcp/mod.rs))

实现 Model Context Protocol 客户端，JSON-RPC 2.0 over stdio：

```
Agent Loop → ToolExecutor → McpToolBridge → McpClient ──stdin──► MCP Server Process
                                                     ◄─stdout── (node/python binary)
```

**核心组件：**
- `McpClient` — 生成子进程，通过 stdin/stdout 通信
- `McpToolBridge` — 将 MCP 工具桥接为 NeeCoder 的 `Tool` trait 实现
- `McpRegistry` — 管理多个 MCP 服务器连接

**工具发现：** 启动时自动连接配置的 MCP 服务器，发现工具后注入 Agent 工具注册表

### 4.18 沙箱安全 ([sandbox/mod.rs](src-tauri/src/sandbox/mod.rs))

三种沙箱模式：

| 模式 | 读权限 | 写权限 |
|------|--------|--------|
| `Strict` | 仅项目路径 + allowed_paths | 仅项目路径 |
| `Permissive` | 无限制 | 仅项目路径 |
| `Disabled` | 无限制 | 无限制 |

**`SandboxChecker` 提供：**
- 路径检查（读/写分离，blocked_paths 最高优先级）
- 命令安全检查（内置黑名单 + 自定义 blocked_commands）
- 文件大小限制（`max_file_size_mb`）
- 域名过滤（`allowed_domains`）
- 细粒度权限（`confirm_write_paths` / `auto_allow_paths` / `auto_allow_commands`）

### 4.19 Skill 系统 ([skill/mod.rs](src-tauri/src/skill/mod.rs))

基于 Markdown 文件的可扩展 Skill 系统：

**Skill 定义：** YAML frontmatter + 模板体
```yaml
---
name: "review-pr"
description: "Review a pull request"
trigger: "/review-pr"
mode: "agent"
agent: "reviewer"
tools: ["read_file", "get_diagnostics"]
---
Review the following PR: $SELECTION
```

**模板变量：** `$SELECTION` / `$FILE_PATH` / `$FILE_CONTENT` / `$PROJECT_PATH` / `$ARGUMENTS` / `$LANGUAGE`

**加载路径：** 全局 `skills/` 目录 + 项目 `.neecoder/skills/` 目录

### 4.20 遥测系统 ([telemetry/mod.rs](src-tauri/src/telemetry/mod.rs))

JSONL 格式的使用分析系统：

**事件类型：** `SessionStart` / `SessionEnd` / `ToolCall` / `Error` / `Compaction`

**存储：** `{app_data}/telemetry/telemetry.jsonl`（独立于日志文件）

**内存计数器：** `AtomicU64` 线程安全实时统计

**查询 API：** `get_summary()` 返回聚合快照

### 4.21 文件系统监听 ([fs_watcher/mod.rs](src-tauri/src/fs_watcher/mod.rs))

基于 `notify` crate，2000ms 防抖窗口，变更类型：`Created` / `Modified` / `Deleted`

### 4.22 配置管理 ([config/mod.rs](src-tauri/src/config/mod.rs))

**`AppSettings`** 核心配置（关键字段）：
```rust
pub struct AppSettings {
    pub llm_provider: LlmProvider,     // Provider 选择
    pub completion_model: String,       // 补全模型
    pub chat_model: String,             // 对话模型
    pub fast_model: String,             // 轻量模型（Ask/摘要）
    pub model_routing_enabled: bool,    // 自动模型路由
    pub api_key: String,                // 运行时 API Key（不落盘）
    pub api_key_encrypted: Option<String>,
    pub sandbox: SandboxConfig,         // 沙箱安全配置
    pub max_api_calls_per_session: u32, // 会话 API 调用上限
    pub loop_no_progress_threshold: u32,
    pub loop_ping_pong_cycles: u32,
    pub loop_failure_streak_threshold: u32,
    pub thinking_enabled: bool,         // Claude Extended Thinking
    pub thinking_budget: u32,
    pub tavily_api_key: String,         // Tavily 搜索 API Key
    // ... 其他字段
}
```

**API Key 加密：** XOR 混淆 + hex 编码，`#[serde(skip_serializing)]` 阻止明文落盘

### 4.23 日志系统 ([logging/mod.rs](src-tauri/src/logging/mod.rs))

`DualLogger` 双输出：控制台（`RUST_LOG` 控制，彩色）+ 文件（`{app_data}/logs/neecoder.log`，自动轮转，保留 5 个历史文件）

### 4.24 命令层 (Tauri Commands)

**16 个命令模块，50+ 个 Tauri 命令：**

| 模块 | 关键命令 | 功能 |
|------|---------|------|
| **config** | `get_settings` / `update_settings` / `get_app_logs` | 配置与日志 |
| **completion** | `request_completion` / `cancel_completion` | FIM 补全 |
| **chat** | `send_message` / `new_session` / `list_sessions` / `cancel_agent` / `answer_agent_question` / `answer_confirm` / `start_cloud_agent` | 对话与 Agent |
| **agent** | `save_agent` / `delete_agent` | 自定义 Agent 管理 |
| **cloud** | 云 Agent 任务启停/查询 | 云 Agent 管理 |
| **a2a** | A2A 远程 Agent 调用 | A2A 协议 |
| **memory** | 记忆浏览/搜索/统计 | 记忆系统 |
| **project** | `open_project` / `get_file_tree` / `read_file` / `write_file` / `accept_change` / `reject_change` | 文件操作 |
| **edit_inline** | `edit_inline` | 内联代码编辑（LLM 驱动） |
| **pty** | `start_terminal` / `write_stdin` / `resize_terminal` / `stop_terminal` | PTY 终端 |
| **lsp** | `start_lsp` / `get_symbols` / `get_hover_info` | LSP |
| **search** | `search_codebase` / `reindex_project` / `get_index_stats` | RAG 搜索 |
| **dependency_graph** | `get_dependency_graph` | 依赖图扫描 |
| **review** | `trigger_auto_review` | 自动代码审查 |
| **mcp** | `list_mcp_servers` / `connect_mcp_server` / `disconnect_mcp_server` | MCP 管理 |
| **skill** | `list_skills` / `execute_skill` | Skill 执行 |

---

## 5. 前端架构 (React + TypeScript)

### 5.1 入口与根组件

**[App.tsx](src/App.tsx)** — 根组件，管理全局状态：
- `activeView` — `editor` / `chat` / `settings` / `search` / `cloud` / `terminal` / `graph` / `memory` / `insights` / `checkpoints` / `timeline`
- `projectPath` / `openFiles` / `activeFile`
- `completionId` / `completionText` — 补全状态
- `showOutline` / `outlineSymbols` — 大纲面板

**布局结构：**
```
┌──────────────────────────────────────────────────┐
│ [Explorer] [Editor Tabs]        [🔍Find] [📑Outline] │
├────────────┬──────────────────┬──────────────────┤
│ File       │ CodeEditor       │ Side Panel       │
│ Explorer   │                  │ (Chat/Search/    │
│            │                  │  Settings)       │
├────────────┴──────────────────┴──────────────────┤
│ Terminal Panel (xterm.js PTY)                    │
├──────────────────────────────────────────────────┤
│ StatusBar [Explorer|Files|...|Cloud|Chat|LLM|Settings] │
└──────────────────────────────────────────────────┘
```

### 5.2 核心组件

**[ChatPanel.tsx](src/components/ChatPanel.tsx)** — 最复杂的前端组件
- 三种模式（Ask / Edit / Agent）+ Agent 选择器
- 会话管理（创建/切换/删除/加载历史）
- Markdown 渲染 + 代码块 diff 预览（EditCodeBlock 组件）
- 工具调用卡片 + Todo 列表 + 日志面板
- 图片输入（粘贴/拖拽/选择器）+ 文件拖拽上下文
- 消息编辑 + 重新生成

**[CodeEditor.tsx](src/components/CodeEditor.tsx)** — CodeMirror 6
- 动态语言扩展 + 幽灵文本补全
- `Tab` 接受 / `Esc` 拒绝 / `Alt+]` 下一候选
- 通过 `window.__neecoder_editor` 暴露 API

**[TerminalPanel.tsx](src/components/TerminalPanel.tsx)** — xterm.js + PTY
- 连接后端 `portable-pty` 的真实终端
- 支持 resize、颜色、交互式程序

**[InlineEdit.tsx](src/components/InlineEdit.tsx)** — LLM 内联编辑
- 选中代码 → 输入指令 → LLM 生成修改 → diff 预览 → Accept/Reject

**[FileExplorer.tsx](src/components/FileExplorer.tsx)** — 懒加载文件树
- 右键菜单 + 双击重命名 + 头部操作按钮

**[Settings.tsx](src/components/Settings.tsx)** — 完整设置界面
- LLM Provider / 模型 / API Key / 补全 / 自定义指令
- MCP 服务器管理面板 + 沙箱配置

**[CloudAgentPanel.tsx](src/components/CloudAgentPanel.tsx)** — 云 Agent 任务管理

**[AgentTimelinePanel.tsx](src/components/AgentTimelinePanel.tsx)** — Agent 执行时间线
- 订阅 `chat-event`，渲染工具调用/结果/重试、思考、状态、检查点、编辑、计划等事件流
- 工具调用成功/失败状态标记、耗时显示、点击展开详情、自动滚动

**[CheckpointPanel.tsx](src/components/CheckpointPanel.tsx)** — 迭代检查点与 diff 面板
- 展示每轮迭代快照（commit hash、文件变更），支持 diff 查看与回滚

**[MemoryPanel.tsx](src/components/MemoryPanel.tsx)** / **[InsightsPanel.tsx](src/components/InsightsPanel.tsx)** — 记忆与遥测面板
- 记忆：长期记忆/MEMORY.md 浏览与搜索
- Insights：遥测统计与 Agent 审计日志（JSONL）

**[DependencyGraph.tsx](src/components/DependencyGraph.tsx)** — 依赖图可视化
- 后端扫描项目依赖关系，cytoscape 渲染

**[SearchPanel.tsx](src/components/SearchPanel.tsx)** — RAG 混合搜索界面

### 5.3 Tauri API 抽象层

**[useTauri.ts](src/hooks/useTauri.ts)** — 30+ 个 API 函数封装
- `isTauri()` 环境检测 + `tryInvoke()` 安全调用
- 非 Tauri 环境返回 Mock 数据

### 5.4 主题与样式系统

**[global.css](src/styles/global.css)** — Catppuccin Mocha 暗色主题，原生 CSS + BEM + CSS 变量

---

## 6. 前后端通信协议

### invoke 命令（请求/响应）
前端 `invoke("command_name", { params })` → 后端 `Result<T, String>`

### Tauri Events（推送）

| 事件名 | 用途 |
|--------|------|
| `chat-event` | 对话流式事件（21 种变体，含 ToolCall/ToolResult/ToolRetry/CheckpointCreated/PlanCreated 等） |
| `completion-event` | 代码补全流式事件 |
| `cloud-agent-event` | 云 Agent 任务状态变更 |
| `pty-output` | 终端输出数据 |
| `pty-exit` | 终端进程退出 |
| `edit-inline-event` | 内联编辑流式事件 |

---

## 7. 数据流与关键路径

### Agent 消息完整流程

```
用户输入 → ChatPanel.sendChatMessage()
  → invoke("send_message", { mode: "Agent" })
    → sanitize_messages() 净化历史
    → inject_memory_context() 注入记忆（MEMORY.md + 笔记 + 艾宾浩斯）
    → tokio::spawn → agent::run_agent()
      → 循环:
        → stream_chat → parse tool_calls
        → pre_tool_hook_chain (Snapshot/Confirm)
        → ToolExecutor.execute() [2min timeout]
        → 失败重试: 可重试错误自动重试 1 次 ([RETRY_FAILED]/[RETRY_SUCCESS] 标记)
        → 死锁检测: 同参工具连续 3 次 → [DEADLOCK_DETECTED] 提示换策略
        → post_tool_hook_chain (Truncate/Filter/ErrorPattern/AutoRollback/TddGate)
        → post_tool_batch_chain (AutoDiagnose/AuditLog)
        → loop_detector.check() → InjectWarning / HardStop
        → checkpoint.create()
        → emit ToolCall/ToolResult/EditDiff/CheckpointCreated...
      → 命令失败时: 系统提示词引导 auto_fix 诊断 → 修复 → 重试（错误自愈循环）
      → 循环结束
    → 持久化消息 + append_note + dreaming
← Events → ChatPanel 实时渲染
```

---

## 8. 安全机制

### 多层安全防护

| 层级 | 机制 | 描述 |
|------|------|------|
| **沙箱** | `SandboxChecker` | 路径读写限制、命令黑名单、文件大小限制、域名过滤 |
| **Hook** | `ConfirmHook` | 危险操作前端确认（60s 超时自动拒绝） |
| **Hook** | `SensitiveDataFilterHook` | 工具输出中的 API Key 自动脱敏 |
| **终端** | `is_dangerous()` | 拦截 `rm -rf /`、`format`、`curl\|sh` 等 |
| **API Key** | XOR + skip_serializing | 运行时明文不落盘，持久化加密 |
| **工具超时** | 2 分钟硬限制 | `tokio::time::timeout` 防止工具挂起 |
| **循环检测** | 4 策略 + 2 级裁决 | 防止 Agent 无限循环 |
| **API 上限** | `max_api_calls_per_session` | 限制单会话 API 调用次数 |

---

## 9. 测试覆盖

| 模块 | 覆盖范围 |
|------|---------|
| Agent Tools | 工具注册、危险命令检测、工具执行 |
| RAG | 代码分块、BM25/向量/混合搜索、SQLite 持久化 |
| Memory | 会话 CRUD、消息存取、长期记忆、笔记、搜索 |
| Completion | FIM prompt 构建、后处理、系统提示词 |
| Sandbox | 路径检查、命令拦截、沙箱模式切换 |
| Skill | 模板渲染、Skill 加载、变量替换 |

---

## 10. 依赖清单

### Rust 后端

| 依赖 | 版本 | 用途 |
|------|------|------|
| tauri | 2 | 桌面应用框架 |
| tauri-plugin-* | 2 | shell/dialog/fs/process/clipboard |
| serde / serde_json / serde_yaml | 1 / 1 / 0.9 | 序列化 |
| tokio | 1 | 异步运行时 |
| reqwest | 0.12 | HTTP 客户端（SSE 流式） |
| rusqlite | 0.32 | SQLite（RAG 索引） |
| tiktoken-rs | 0.5 | 精确 token 计数 |
| portable-pty | 0.8 | 跨平台 PTY 终端 |
| notify | 7 | 文件系统监听 |
| regex / glob | 1 / 0.3 | 模式匹配 |
| uuid / chrono | 1 / 0.4 | UUID / 日期时间 |
| strsim | 0.11 | 字符串相似度 |
| clap | 4 | CLI 参数解析 |
| async-trait | 0.1 | 异步 trait |
| anyhow / thiserror | 1 / 2 | 错误处理 |

### 前端

| 依赖 | 用途 |
|------|------|
| react / react-dom | UI 框架 |
| react-markdown | Markdown 渲染 |
| @codemirror/* | 代码编辑器 |
| @xterm/xterm + addon-fit | 终端模拟 |
| @tauri-apps/api + plugin-* | Tauri 前端 API |
| react-syntax-highlighter | 语法高亮 |
| lucide-react | 图标库 |
| vite / typescript | 构建 / 类型检查 |
