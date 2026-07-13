# NeeCoder 项目架构文档

> NeeCoder 是一款基于 **Tauri 2.0 + React + Rust** 构建的 AI 编程助手桌面应用，集成了代码编辑器、AI 对话面板、智能代码补全、RAG 代码搜索、多 Agent 协作和 LSP 语言服务器协议等功能。

---

## 目录

1. [总体架构](#1-总体架构)
2. [技术栈](#2-技术栈)
3. [目录结构](#3-目录结构)
4. [后端架构 (Rust)](#4-后端架构-rust)
   - 4.1 [应用入口与生命周期](#41-应用入口与生命周期-librs)
   - 4.2 [LLM 通信层](#42-llm-通信层-llmmodrs)
   - 4.3 [Agent 系统](#43-agent-系统-agentmodrs)
   - 4.4 [Agent 工具集](#44-agent-工具集-agenttoolsmodrs)
   - 4.5 [子 Agent 调度](#45-子-agent-调度-agentsub_agentrs)
   - 4.6 [Agent 定义与注册](#46-agent-定义与注册-agentdefinitionrs)
   - 4.7 [对话管理](#47-对话管理-chatmodrs)
   - 4.8 [记忆系统](#48-记忆系统-memorymodrs)
   - 4.9 [代码补全](#49-代码补全-completionmodrs)
   - 4.10 [RAG 代码索引与搜索](#410-rag-代码索引与搜索-ragmodrs)
   - 4.11 [LSP 语言服务器](#411-lsp-语言服务器-lspmodrs)
   - 4.12 [文件系统监听](#412-文件系统监听-fs_watchermodrs)
   - 4.13 [配置管理](#413-配置管理-configmodrs)
   - 4.14 [日志系统](#414-日志系统-loggingmodrs)
   - 4.15 [命令层 (Tauri Commands)](#415-命令层-tauri-commands-commands)
5. [前端架构 (React + TypeScript)](#5-前端架构-react--typescript)
   - 5.1 [入口与根组件](#51-入口与根组件)
   - 5.2 [核心组件](#52-核心组件)
   - 5.3 [Tauri API 抽象层](#53-tauri-api-抽象层)
   - 5.4 [主题与样式系统](#54-主题与样式系统)
6. [前后端通信协议](#6-前后端通信协议)
7. [数据流与关键路径](#7-数据流与关键路径)
8. [安全机制](#8-安全机制)
9. [测试覆盖](#9-测试覆盖)
10. [依赖清单](#10-依赖清单)

---

## 1. 总体架构

```
┌──────────────────────────────────────────────────────────┐
│                    Tauri 2.0 桌面应用                      │
├────────────────────────┬─────────────────────────────────┤
│    前端 (React + TS)    │         后端 (Rust)              │
│  ┌──────────────────┐  │  ┌────────────────────────────┐ │
│  │ App.tsx (根组件)  │  │  │ Commands (Tauri invoke)    │ │
│  ├──────────────────┤  │  ├────────────────────────────┤ │
│  │ ChatPanel        │  │  │ Agent 系统 (19 工具)       │ │
│  │ CodeEditor       │◄─┼─►│ LLM 通信层 (多 Provider)   │ │
│  │ FileExplorer     │  │  │ RAG 代码索引 (BM25+向量)   │ │
│  │ SearchPanel      │  │  │ 记忆系统 (会话/长期/笔记)   │ │
│  │ Settings         │  │  │ LSP 语言服务器              │ │
│  │ StatusBar        │  │  │ 代码补全 (FIM)              │ │
│  └──────────────────┘  │  │ 文件监听 (notify)           │ │
│  useTauri.ts (API 层)  │  │ 配置管理 (XOR 加密)         │ │
├────────────────────────┤  └────────────────────────────┘ │
│  Tauri Event System    │  Tauri State + Invoke Handler    │
└────────────────────────┴─────────────────────────────────┘
```

**核心通信方式：**
- **invoke (请求/响应)**：前端调用后端命令（如 `send_message`, `read_file`）
- **Tauri Events (推送)**：后端向前端推送流式事件（如 `chat-event`, `completion-event`）
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
| 异步运行时 | Tokio | Rust 异步任务、流式处理 |
| HTTP 客户端 | reqwest | LLM API 调用（流式 SSE） |
| 数据库 | SQLite (rusqlite) | RAG 代码索引持久化存储 |
| 文件监听 | notify | 文件系统变更检测与自动重索引 |
| 序列化 | serde + serde_json + serde_yaml | JSON/YAML 序列化 |
| 日志 | log + 自定义 DualLogger | 双输出日志（控制台 + 文件轮转） |

---

## 3. 目录结构

```
NeeCoder/
├── src/                          # 前端源码
│   ├── main.tsx                  # React 入口
│   ├── App.tsx                   # 根组件（布局、文件标签页管理）
│   ├── components/
│   │   ├── ChatPanel.tsx         # AI 对话面板（~1796 行，最复杂组件）
│   │   ├── CodeEditor.tsx        # CodeMirror 6 编辑器封装
│   │   ├── FileExplorer.tsx      # 文件树浏览器
│   │   ├── SearchPanel.tsx       # 代码搜索面板
│   │   ├── Settings.tsx          # 设置界面
│   │   ├── StatusBar.tsx         # 底部状态栏
│   │   ├── ContextMenu.tsx       # 右键菜单
│   │   ├── MentionMenu.tsx       # @ 文件提及菜单
│   │   ├── Overlay.tsx           # 全局遮罩层（Agent 提问/确认）
│   │   ├── CloudAgentPanel.tsx   # 云 Agent 后台任务管理面板
│   │   └── SyntaxHighlighterWrapper.tsx  # 语法高亮封装
│   ├── hooks/
│   │   └── useTauri.ts           # Tauri API 统一抽象层（~500 行）
│   └── styles/
│       └── global.css            # Catppuccin Mocha 暗色主题
├── src-tauri/                    # 后端源码
│   ├── src/
│   │   ├── main.rs               # 二进制入口
│   │   ├── lib.rs                # Tauri 应用构建器、State 注册、命令注册
│   │   ├── agent/                # Agent 系统
│   │   │   ├── mod.rs            # Agent 主循环（1249 行）
│   │   │   ├── tools/            # 19 个 Agent 工具实现
│   │   │   ├── definition.rs     # Agent 定义与注册表
│   │   │   ├── sub_agent.rs      # 子 Agent 调度
│   │   │   └── utils.rs          # 辅助函数
│   │   ├── chat/mod.rs           # 对话消息模型与事件定义
│   │   ├── commands/             # Tauri 命令（前端可调用的 API）
│   │   │   ├── chat.rs           # 对话相关命令
│   │   │   ├── config.rs         # 配置相关命令
│   │   │   ├── completion.rs     # 补全相关命令
│   │   │   ├── project.rs        # 项目/文件操作命令
│   │   │   ├── lsp.rs            # LSP 相关命令
│   │   │   └── search.rs         # 搜索/索引命令
│   │   ├── completion/mod.rs     # FIM 补全 prompt 构建
│   │   ├── config/mod.rs         # 配置管理（XOR 加密 API Key）
│   │   ├── fs_watcher/mod.rs     # 文件系统监听
│   │   ├── llm/mod.rs            # LLM API 通信（808 行）
│   │   ├── logging/mod.rs        # 双输出日志系统
│   │   ├── lsp/mod.rs            # LSP 协议实现（650 行）
│   │   ├── memory/               # 记忆系统
│   │   │   ├── mod.rs            # MemoryManager 统一接口
│   │   │   ├── session_store.rs  # Markdown 会话存储
│   │   │   ├── long_term.rs      # 长期记忆（MEMORY.md）
│   │   │   ├── notes.rs          # 每日笔记
│   │   │   ├── search.rs         # 记忆搜索
│   │   │   └── tools.rs          # 记忆工具（供 Agent 使用）
│   │   └── rag/mod.rs            # RAG 代码索引（776 行）
│   ├── tools.json                # Agent 工具 JSON Schema 定义
│   └── Cargo.toml                # Rust 依赖清单
```

---

## 4. 后端架构 (Rust)

### 4.1 应用入口与生命周期 ([lib.rs](file:///d:/workspace/NeeCoder/src-tauri/src/lib.rs))

`lib.rs` 是整个后端的核心入口，负责：

1. **初始化日志系统** — 调用 `logging::init()` 设置双输出日志
2. **注册 Tauri 插件** — shell、dialog、fs、process、clipboard
3. **初始化全局 State** — 通过 `app.manage()` 注册以下共享状态：
   - `ConfigState` — 配置管理器（`Arc<RwLock<ConfigManager>>`）
   - `AppSettings` — 当前设置（`Arc<RwLock<AppSettings>>`）
   - `ChatState` — 对话记忆（`Arc<RwLock<ConversationMemory>>`）
   - `CompletionCache` — 补全 LRU 缓存（最大 200 条）
   - `CancelMap` — 补全取消标志
   - `AgentCancelMap` — Agent 取消标志
   - `FileSnapshots` — 编辑快照（用于 accept/reject）
   - `LspManager` — LSP 管理器
   - `CodeIndexer` — RAG 代码索引器（启动时从 SQLite 加载）
   - `QuestionAwaiters` — Agent 提问等待通道
   - `ConfirmAwaiters` — 危险操作确认等待通道
   - `ToolRegistry` — 工具定义注册表（从 `tools.json` 加载）
   - `AgentRegistry` — Agent 定义注册表（从 `agents.json` 加载）
   - `FileWatcher` — 文件系统监听器
4. **启动后台任务** — 2 秒轮询文件变更事件，自动重索引修改的源码文件并持久化到 SQLite
5. **注册 invoke_handler** — 暴露 40+ 个 Tauri 命令供前端调用

**关键设计决策：** 所有全局状态使用 `Arc<RwLock<T>>` 或 `Arc<Mutex<T>>` 包装，支持并发读写。`RwLock`（Tokio）用于读多写少的场景（如 Settings），`std::sync::Mutex` 用于简单的互斥场景。

### 4.2 LLM 通信层 ([llm/mod.rs](file:///d:/workspace/NeeCoder/src-tauri/src/llm/mod.rs))

提供统一的 LLM API 调用接口，支持 **4 种 Provider**：

| Provider | 用途 | 默认 Base URL |
|----------|------|---------------|
| OpenAI | 通用 | `api.openai.com/v1` |
| DeepSeek | 默认 Provider | `api.deepseek.com/v1` |
| Anthropic | Claude 系列 | `api.anthropic.com/v1` |
| Ollama | 本地部署 | `localhost:11434` |

**两种调用模式：**
- **`stream_fim`** — Fill-in-the-Middle 补全模式，用于代码补全
- **`stream_chat`** — 对话模式，支持工具调用（function calling）

**核心数据类型：**
- `ChatMessage` — 包含 `role`、`content`、可选 `tool_calls`、可选 `tool_call_id`
- `FimRequest` — FIM 补全请求
- `ChatRequestParams` — 对话请求参数（model、messages、system、max_tokens、temperature）
- `CancelFlag` — `Arc<AtomicBool>` 用于取消流式请求

**流式处理：** 使用 `reqwest` 的 SSE（Server-Sent Events）流式读取，每收到一个 token 就通过回调函数 `on_token` 传递给调用方，调用方再通过 Tauri Event 推送到前端。

### 4.3 Agent 系统 ([agent/mod.rs](file:///d:/workspace/NeeCoder/src-tauri/src/agent/mod.rs))

Agent 是 NeeCoder 的核心 AI 执行引擎，采用**迭代循环**模式：

```
用户消息 → LLM 推理 → 工具调用 → 工具结果反馈 → LLM 再推理 → ... → 完成
```

**AgentInstance 结构：**
```
AgentInstance
├── messages: Vec<ChatMessage>      # 对话历史
├── provider/api_key/base_url       # LLM 配置
├── chat_model                      # 使用的模型
├── project_path                    # 当前项目路径
├── custom_instructions             # 自定义指令
├── cancelled: AtomicBool           # 取消标志
├── question_awaiters               # 提问等待通道
├── confirm_awaiters                # 确认等待通道
└── iteration: u32                  # 当前迭代次数
```

**主循环流程 (`run_agent` → `run_no_dispatch`)：**
1. 将用户消息追加到对话历史
2. 调用 `stream_chat` 向 LLM 发送请求
3. 解析 LLM 响应中的 `tool_calls`
4. 对每个工具调用：
   - **危险操作确认** — `delete_file`/`delete_directory`/`run_terminal_command` 需要用户确认
   - 通过 `ToolExecutor` 执行工具
   - 检查 `PostExecuteAction`（更新 Todo、提问用户、调度子 Agent）
   - 将工具结果作为 `Tool` 角色消息追加到历史
5. 将完整的工具调用 + 结果发射给前端（`ToolCall`/`ToolResult` 事件）
6. 重复步骤 2-5，直到 LLM 不再调用工具或达到最大迭代次数

**关键安全机制：**
- **危险操作确认** — 3 种工具（删除文件/目录、执行终端命令）需要用户在前端弹窗中点击 "Allow" 才能执行
- **超时自动拒绝** — 确认请求 60 秒无响应自动拒绝
- **取消支持** — 通过 `AtomicBool` 标志支持用户中途取消

**Agent 可观测性事件（全部通过 Tauri Events 发射到前端）：**
- `AgentThinking` — LLM 的思考过程
- `AgentStatus` — 迭代进度 + Token 用量估算
- `ToolCall` / `ToolResult` — 工具调用与结果（含执行耗时 `duration_ms`）
- `ToolRetry` — 工具失败后自动重试通知
- `TodoUpdate` — 任务列表更新
- `AskUserQuestion` — Agent 向用户提问
- `ConfirmRequest` — 危险操作确认请求
- `ContextTrimmed` — 上下文裁剪通知
- `AgentLog` — Agent 运行日志

### 4.4 Agent 工具集 ([agent/tools/mod.rs](file:///d:/workspace/NeeCoder/src-tauri/src/agent/tools/mod.rs))

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
- `project_path` — 项目根目录
- `indexer` — 代码索引器（供 `search_codebase` 使用）

**`PostExecuteAction`** — 工具执行后的特殊动作（避免字符串匹配）：
- `None` — 无特殊动作
- `UpdateTodos` — 更新 Todo 列表
- `AskUser` — 向用户提问
- `DispatchAgent` — 调度单个子 Agent
- `DispatchAgents` — 调度多个子 Agent

**19 个工具一览：**

| 工具 | 功能 | 需要确认 |
|------|------|---------|
| `read_file` | 读取文件内容 | 否 |
| `write_file` | 写入/创建文件 | 否 |
| `append_file` | 追加内容到文件末尾 | 否 |
| `delete_file` | 删除文件 | **是** |
| `list_directory` | 列出目录内容 | 否 |
| `create_directory` | 创建目录（含父目录） | 否 |
| `delete_directory` | 递归删除目录 | **是** |
| `get_symbols` | 提取文件中的符号定义 | 否 |
| `search_codebase` | RAG 语义代码搜索 | 否 |
| `grep` | 文本模式搜索（大小写不敏感） | 否 |
| `run_terminal_command` | 执行 shell 命令 | **是** |
| `glob` | glob 模式文件查找 | 否 |
| `edit` | 精确字符串替换（首选编辑方式） | 否 |
| `todo_write` | 创建/更新任务列表 | 否 |
| `web_search` | DuckDuckGo 网页搜索 | 否 |
| `web_fetch` | 获取网页内容（HTML 转纯文本） | 否 |
| `ask_user_question` | 向用户提问并阻塞等待回答 | 否 |
| `get_diagnostics` | 获取编译器/linter 诊断信息 | 否 |
| `dispatch_agent` / `dispatch_agents` | 调度子 Agent | 否 |

**`ToolExecutor`** — 工具注册表与调度器：
- `register()` — 注册工具实例到 `HashMap<String, Arc<dyn Tool>>`
- `get()` — 按名称查找工具
- `post_execute_action()` — 获取工具执行后需要的特殊动作
- `build_executor()` — 工厂函数，注册全部 19 个工具

### 4.5 子 Agent 调度 ([agent/sub_agent.rs](file:///d:/workspace/NeeCoder/src-tauri/src/agent/sub_agent.rs))

支持两种调度模式：

- **`run_sub_agent`** — 串行执行单个子 Agent，调用 `run_no_dispatch()`（不支持嵌套调度，避免类型循环）
- **`run_sub_agents_parallel`** — 多 Agent 调度，包含**文件冲突检测**（同一文件被多个 Agent 修改时发出警告），然后串行执行

**子 Agent 结果格式：**
- 成功：`[SUB_AGENT_RESULT:{agent_id}]\n{result}`
- 失败：`[SUB_AGENT_ERROR:{agent_id}]\n{error}`

### 4.6 Agent 定义与注册 ([agent/definition.rs](file:///d:/workspace/NeeCoder/src-tauri/src/agent/definition.rs))

`AgentDefinition` 描述一个 Agent 的完整配置：

```rust
pub struct AgentDefinition {
    pub id: String,              // 唯一标识
    pub name: String,            // 显示名称
    pub description: String,     // 描述
    pub system_prompt: String,   // 系统提示词
    pub tool_names: Vec<String>, // 可用工具列表
    pub model: Option<String>,   // 可选自定义模型
    pub temperature: Option<f32>,// 可选自定义温度
    pub max_iterations: Option<usize>, // 最大迭代次数
    pub max_tokens: Option<u32>, // 最大 token 数
}
```

**4 个内置 Agent：**

| Agent | 角色 | 可用工具 | 迭代上限 |
|-------|------|---------|---------|
| `orchestrator` | 主 Agent / 调度器 | 全部 19 个工具 + dispatch | 15 |
| `code_writer` | 代码编写专家 | 文件读写 + 编辑 + 搜索 | 10 |
| `debugger` | 调试专家 | 读文件 + 搜索 + 终端 + 诊断 | 8 |
| `reviewer` | 代码审查（只读） | 读文件 + 搜索 + 诊断 | 5 |

**加载优先级：** `agents.json` 文件（运行时可修改）→ 内嵌默认定义

### 4.7 对话管理 ([chat/mod.rs](file:///d:/workspace/NeeCoder/src-tauri/src/chat/mod.rs))

定义对话的核心数据模型：

**消息模型：**
- `ChatMessage` — `{ role, content, tool_calls }`
- `Role` — `User` / `Assistant` / `System` / `Tool`
- `ToolCall` — `{ id, tool_name, arguments, timestamp }`

**对话请求：**
- `ChatRequest` — `{ messages, context, mode }`
- `ChatContext` — 包含 `active_file`、`selected_code`、`file_mentions`（@文件）、`symbol_mentions`
- `ChatMode` — `Ask`（问答）/ `Edit`（编辑建议）/ `Agent`（自主执行）

**事件枚举 `ChatEvent`（14 种变体）：**
`Started` → `Delta`（流式 token）→ `Finished` → `ToolCall` → `ToolResult` → `ToolRetry` → `TodoUpdate` → `AskUserQuestion` → `ConfirmRequest` → `AgentThinking` → `AgentStatus` → `AgentLog` → `ContextTrimmed` → `Error` / `Cancelled`

**三种模式的处理逻辑（在 [commands/chat.rs](file:///d:/workspace/NeeCoder/src-tauri/src/commands/chat.rs) 中）：**

- **Ask 模式** — 直接调用 `stream_chat`，流式返回答案
- **Edit 模式** — 使用专用 `EDIT_SYSTEM_PROMPT`，返回代码变更建议
- **Agent 模式** — `tokio::spawn` 后台任务，调用 `agent::run_agent` 执行完整工具循环。完成后执行：
  1. 持久化对话消息到 Memory
  2. 追加笔记（Agent 完成摘要）
  3. 触发 **Dreaming**（LLM 摘要 → 写入 `MEMORY.md`）

**消息净化 `sanitize_messages`：** 确保发送给 LLM 的消息格式合法：
- 非 Agent 模式：移除所有 tool/tool_calls 消息
- Agent 模式：确保 tool_calls → tool 消息配对正确，处理孤立消息

### 4.8 记忆系统 ([memory/mod.rs](file:///d:/workspace/NeeCoder/src-tauri/src/memory/mod.rs))

`MemoryManager` 是统一的记忆管理接口，包含 4 个子系统：

**1. 会话存储 (`session_store.rs`)**
- 基于 **Markdown 文件**的持久化方案
- 每个会话 = 一个目录：`sessions/{uuid}/session.md` + `sessions/{uuid}/messages/00000001.md`
- 消息文件使用 YAML frontmatter 存储 `role` 和 `tool_calls`
- 支持创建/加载/清理/删除会话

**2. 长期记忆 (`long_term.rs`)**
- 单一文件 `MEMORY.md`
- 支持读取、覆写、按章节追加
- 在 Agent 模式启动时注入到系统提示词中
- Agent 完成后通过 **Dreaming** 自动由 LLM 生成摘要并追加

**3. 每日笔记 (`notes.rs`)**
- 按日期存储：`notes/YYYY-MM-DD.md`
- 每条笔记带时间戳前缀：`- [HH:MM:SS] 内容`
- Agent 完成后自动追加摘要到当天笔记

**4. 记忆搜索 (`search.rs`)**
- 递归搜索所有 `.md` 文件
- 简单的相关性评分：精确匹配(10分) > 前缀匹配(5分) > 包含(1分)
- 跳过会话消息文件（`messages/` 目录）

**记忆注入机制：**
- `inject_memory_context()` — 在 Agent 启动前，将 `MEMORY.md` 和近期笔记注入到 `custom_instructions` 中
- `dreaming()` — Agent 完成后，用 LLM 对整个会话生成摘要，追加到 `MEMORY.md`

### 4.9 代码补全 ([completion/mod.rs](file:///d:/workspace/NeeCoder/src-tauri/src/completion/mod.rs))

采用 **FIM (Fill-in-the-Middle)** 范式进行代码补全：

**上下文采集 (`CompletionContext`)：**
- `prefix` / `suffix` — 光标前后的代码
- `imports` — 文件中的 import 语句
- `enclosing_fn` — 光标所在函数的签名
- `cursor_line` / `cursor_column` — 光标位置

**FIM Prompt 构建 (`build_fim_prompt`)：**
```
Language: rust
--- Imports ---
use std::sync::Arc;
--- End Imports ---
--- Context: fn main() ---
--- Code ---
<PRE>
{prefix}
<SUF>
{suffix}
<MID>
```

**后处理 (`post_process_completion`)：** 清理 LLM 生成的补全文本 — 去除前导空白行、尾随空白、修正缩进

**LRU 缓存 (`CompletionCache`)：** 基于 `(file_path, prefix_tail_80, suffix_head_40)` 的哈希键缓存补全结果，最多 200 条

**流式事件：** `Started` → `Delta`（逐 token）→ `Finished` / `Error` / `Cancelled`

### 4.10 RAG 代码索引与搜索 ([rag/mod.rs](file:///d:/workspace/NeeCoder/src-tauri/src/rag/mod.rs))

`CodeIndexer` 是 RAG 搜索引擎的核心，实现**混合搜索**（向量相似度 + BM25 关键词）：

**代码分块 (`CodeChunk`)：**
```rust
pub struct CodeChunk {
    pub id: String,
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub language: String,
    pub chunk_type: ChunkType,  // File/Function/Class/Module/Block
    pub content: String,
    pub summary: String,
}
```

**索引流程：**
1. 遍历项目目录，过滤支持的文件扩展名（30+ 种）
2. 对每个文件调用 `chunk_code()` 按函数/类/模块进行结构化分块
3. 对每个 chunk 调用 LLM 生成 embedding 向量
4. 存储在内存中（`Vec<IndexedChunk>`）+ 文件索引映射（`file_map`）

**持久化：**
- `save_to_db()` — 将全部 chunks 序列化后存入 SQLite
- `load_from_db()` — 启动时从 SQLite 加载已索引数据

**搜索模式：**
- `search()` — 向量相似度搜索（embedding 余弦相似度）
- `bm25_search()` — BM25 关键词搜索（预计算 df_map 优化性能）
- `hybrid_search()` — 混合搜索（向量 60% + BM25 40% 加权融合）

**自动重索引：** `lib.rs` 中的后台循环每 2 秒轮询文件变更，自动对修改的源码文件重新索引并持久化

### 4.11 LSP 语言服务器 ([lsp/mod.rs](file:///d:/workspace/NeeCoder/src-tauri/src/lsp/mod.rs))

`LspManager` 管理多个语言的 LSP 客户端实例：

**支持的 10 种语言：**
Rust（rust-analyzer）、TypeScript/JavaScript（typescript-language-server）、Python（pylsp）、Go（gopls）、Java（jdtls）、C/C++（clangd）、Ruby（solargraph）、PHP（intelephense）、C#（omnisharp）、Kotlin（kotlin-language-server）

**实现的 LSP 协议方法：**
- `start_lsp` — 启动语言服务器进程（stdin/stdout 通信）
- `did_open` / `did_change` / `did_close` — 文档生命周期通知
- `get_symbols` — 获取文件中的符号定义（Document Symbols）
- `get_hover` — 获取悬停信息（Hover）
- `shutdown_all` — 关闭所有 LSP 客户端

**内部通信：** 使用 JSON-RPC 2.0 over stdin/stdout，`AtomicU64` 请求 ID 计数器

### 4.12 文件系统监听 ([fs_watcher/mod.rs](file:///d:/workspace/NeeCoder/src-tauri/src/fs_watcher/mod.rs))

基于 `notify` crate 实现的文件系统监听器：

**核心功能：**
- `start_watch()` — 启动对指定路径的递归/非递归监听
- `stop_watch()` — 停止监听特定路径
- `poll_events()` — 轮询式获取变更事件（带防抖）
- `on_change()` — 注册变更回调

**防抖机制：** 同一文件的多次变更合并为一次事件，默认 2000ms 防抖窗口。使用 `DebouncedEvent` 跟踪每个文件的最新变更类型和时间戳。

**变更类型：** `Created` / `Modified` / `Deleted`

**用途：** 在 `lib.rs` 中被后台循环调用，自动触发代码重索引。

### 4.13 配置管理 ([config/mod.rs](file:///d:/workspace/NeeCoder/src-tauri/src/config/mod.rs))

**`AppSettings`** 核心配置结构：
```rust
pub struct AppSettings {
    pub llm_provider: LlmProvider,    // Provider 选择
    pub completion_model: String,      // 补全模型
    pub chat_model: String,            // 对话模型
    pub embedding_model: String,       // Embedding 模型
    pub api_key: String,               // 运行时 API Key（不落盘）
    pub api_key_encrypted: Option<String>, // 加密 API Key（持久化）
    pub completion_enabled: bool,      // 是否启用补全
    pub trigger_debounce_ms: u64,      // 补全触发防抖
    pub max_context_tokens: u32,       // 最大上下文 token
    pub custom_instructions: String,   // 自定义指令
    pub project_paths: Vec<String>,    // 项目路径列表
    pub theme: Theme,                  // Light/Dark
}
```

**API Key 加密方案：** XOR 混淆 + hex 编码
- `xor_obfuscate()` — 加密：逐字节 XOR + hex 编码
- `xor_deobfuscate()` — 解密：hex 解码 + 逐字节 XOR
- `api_key` 使用 `#[serde(skip_serializing)]` 阻止明文落盘
- 启动时自动迁移旧配置（检测明文 → 加密 → 保存）

**`ConfigManager`** — 配置的加载/保存管理器，使用 JSON 格式存储到应用配置目录

### 4.14 日志系统 ([logging/mod.rs](file:///d:/workspace/NeeCoder/src-tauri/src/logging/mod.rs))

自定义 `DualLogger` 实现双输出日志：

**控制台输出：**
- 受 `RUST_LOG` 环境变量控制（默认 `info` 级别）
- 彩色输出（Error=红、Warn=黄、Info=青、Debug=白、Trace=灰）

**文件输出：**
- 路径：`{app_data}/logs/neecoder.log`
- 始终记录 `debug` 及以上级别
- 启动时自动轮转：当前日志 → `neecoder.{timestamp}.log`
- 最多保留 5 个历史日志文件

**外部接口：**
- `get_app_logs` 命令 — 读取最近 N 行日志供前端显示
- `get_log_path` 命令 — 返回日志文件路径

### 4.15 命令层 (Tauri Commands) ([commands/](file:///d:/workspace/NeeCoder/src-tauri/src/commands))

**6 个命令模块**，共 **40+ 个 Tauri 命令**：

| 模块 | 命令 | 功能 |
|------|------|------|
| **config** | `get_settings` | 获取当前设置 |
| | `update_settings` | 更新设置 |
| | `get_app_logs` | 读取应用日志 |
| | `get_log_path` | 获取日志文件路径 |
| **completion** | `request_completion` | 请求 FIM 补全（流式） |
| | `cancel_completion` | 取消正在进行的补全 |
| **chat** | `send_message` | 发送消息（Ask/Edit/Agent 三种模式） |
| | `send_message`（扩展） | 支持 `images` 参数（多模态图片输入） |
| | `new_session` | 创建新会话 |
| | `list_sessions` | 列出所有会话 |
| | `delete_session` | 删除会话 |
| | `get_session_messages` | 获取会话消息历史 |
| | `clear_session` | 清空会话 |
| | `cancel_agent` | 取消正在运行的 Agent |
| | `get_agents` | 获取 Agent 定义列表 |
| | `answer_agent_question` | 回答 Agent 提问 |
| | `answer_confirm` | 确认/拒绝危险操作 |
| | `get_terminal_history` | 获取终端命令历史 |
| | `get_error_summary` | 获取最近错误摘要 |
| | `start_cloud_agent` | 启动后台云 Agent 任务 |
| | `list_cloud_tasks` | 列出所有云 Agent 任务 |
| | `get_cloud_task` | 获取单个云 Agent 任务状态 |
| | `cancel_cloud_task` | 取消云 Agent 任务 |
| **project** | `open_project` | 打开项目 |
| | `get_file_tree` | 获取文件树 |
| | `read_file` | 读取文件 |
| | `write_file` | 写入文件 |
| | `create_file` | 创建文件 |
| | `create_directory` | 创建目录 |
| | `delete_file` | 删除文件/目录 |
| | `rename_file` | 重命名文件 |
| | `accept_change` | 接受 Agent 编辑 |
| | `reject_change` | 拒绝 Agent 编辑（恢复快照） |
| **lsp** | `start_lsp` | 启动语言服务器 |
| | `get_symbols` | 获取文件符号 |
| | `get_hover_info` | 获取悬停信息 |
| | `lsp_did_open/change/close` | LSP 文档生命周期 |
| | `shutdown_lsp` | 关闭所有 LSP |
| **search** | `search_codebase` | 语义搜索代码库 |
| | `reindex_project` | 重新索引项目 |
| | `index_file` | 索引单个文件 |
| | `remove_from_index` | 从索引中移除文件 |
| | `get_index_stats` | 获取索引统计 |
| **mcp** | `list_mcp_servers` | 列出所有 MCP 服务器 |
| | `connect_mcp_server` | 连接 MCP 服务器 |
| | `disconnect_mcp_server` | 断开 MCP 服务器 |

---

## 5. 前端架构 (React + TypeScript)

### 5.1 入口与根组件

**[main.tsx](file:///d:/workspace/NeeCoder/src/main.tsx)** — React 入口，挂载 `App` 组件，导入全局样式

**[App.tsx](file:///d:/workspace/NeeCoder/src/App.tsx)** — 根组件，管理全局状态：

**核心状态：**
- `activeView` — 当前活动视图（`editor` / `chat` / `settings` / `search` / `cloud`）
- `projectPath` — 当前项目路径
- `openFiles` — 打开的文件列表（标签页）
- `activeFile` — 当前活动文件
- `completionId` / `completionText` — 代码补全状态
- `showOutline` / `outlineSymbols` — 编辑器大纲面板状态

**布局结构：**
```
┌──────────────────────────────────────────┐
│ [Explorer] [Editor Tabs]         [🔍Find] [📑Outline] │
├────────────┬──────────────┬──────────────┤
│ File       │ CodeEditor   │ Side Panel   │
│ Explorer   │              │ (Chat/       │
│            │              │  Search/     │
│            │              │  Settings)   │
├────────────┴──────────────┴──────────────┤
│ StatusBar [Explorer|Files|...│Cloud|Chat|LLM|Settings] │
└──────────────────────────────────────────┘
```

**补全事件处理：** 通过 `listen("completion-event")` 监听后端推送的流式 token，累积显示幽灵文本（Ghost Text），用户按 `Tab` 接受，`Esc` 拒绝。

**编辑器工具栏：** 编辑器头部新增 🔍 查找按钮（调用 CodeMirror `openSearchPanel`）和 📑 大纲按钮（调用 LSP `get_symbols` 获取文件符号列表，点击跳转到对应行）。

**大纲面板：** 当打开文件时，可在编辑器右侧展开大纲面板，通过 LSP 获取函数/类/接口等符号定义，按类型显示图标，点击导航到符号所在行。

### 5.2 核心组件

**[ChatPanel.tsx](file:///d:/workspace/NeeCoder/src/components/ChatPanel.tsx)**（~1796 行，最复杂的前端组件）
- 三种聊天模式切换（Ask / Edit / Agent）
- Agent 选择器（从 `agents.json` 加载）
- 会话管理（创建/切换/删除/加载历史）
- Markdown 渲染（`react-markdown` + 语法高亮）
- 代码块 "一键复制" 和 "应用更改" 按钮
- 工具调用卡片（展示执行状态、耗时）
- Todo 列表实时显示
- `@文件` 提及菜单（MentionMenu）
- Agent 提问对话框（AskQuestionOverlay）
- 危险操作确认对话框（ConfirmDangerDialog）
- 流式 token 累积显示
- 日志面板
- **🖼️ 图片输入**：支持粘贴（Ctrl+V）、拖拽、文件选择器三种方式添加图片，base64 编码传递
- **📎 文件拖拽**：从文件管理器拖拽代码文件到 Chat 面板，自动匹配项目文件列表并附加为上下文
- **✏️ 消息编辑**：hover 用户消息可编辑，修改后重新发送（Ctrl+Enter 提交）
- **🔄 重新生成**：最后一条助手回复支持一键重新生成

**[CodeEditor.tsx](file:///d:/workspace/NeeCoder/src/components/CodeEditor.tsx)**
- 基于 CodeMirror 6 构建
- 动态语言扩展（根据文件后缀选择语法高亮）
- 幽灵文本装饰器（Ghost Text Widget）
- 键盘事件拦截（`Tab` 接受、`Esc` 拒绝、`Alt+]` 下一个候选）
- 文件内搜索（`Ctrl+F`，集成 CodeMirror `searchKeymap`）
- 通过 `window.__neecoder_editor` 暴露 `getCursor` / `getContext` / `openFind` / `goToLine` / `getFilePath` / `insertCompletion` 给外部调用

**[FileExplorer.tsx](file:///d:/workspace/NeeCoder/src/components/FileExplorer.tsx)**
- 懒加载文件树（展开目录时请求子节点）
- 文件图标映射（按扩展名显示 emoji）
- **右键上下文菜单**：新建文件/文件夹、重命名、删除（带确认对话框）
- **双击重命名**：双击文件名直接进入内联重命名模式
- **头部操作按钮**：📄+ 新建文件 / 📁+ 新建文件夹 / ↻ 刷新
- **错误提示**：操作失败时显示红色错误横幅

**[StatusBar.tsx](file:///d:/workspace/NeeCoder/src/components/StatusBar.tsx)**
- 底部导航栏：Explorer | 文件数 | 项目名 | Search | ☁️ Cloud | Chat | LLM 状态 | Settings

**[Settings.tsx](file:///d:/workspace/NeeCoder/src/components/Settings.tsx)**
- LLM Provider 配置（OpenAI / DeepSeek / Anthropic / Ollama）
- 模型参数设置（补全模型、对话模型、Embedding 模型）
- API Key 管理与加密
- 代码补全开关与防抖时间
- 自定义指令（Custom Instructions）
- **🔌 MCP 服务器管理面板**：
  - 已连接服务器列表（含连接状态指示灯）
  - 添加新服务器（名称 + JSON-RPC 2.0 URL）
  - 一键断开/重连
  - 服务器状态实时检测

**[CloudAgentPanel.tsx](file:///d:/workspace/NeeCoder/src/components/CloudAgentPanel.tsx)**
- 云 Agent 后台任务管理面板（通过状态栏 ☁️ Cloud 按钮进入）
- 任务列表展示：状态（Pending/Running/Completed/Failed）、消息预览、时间戳
- 自动刷新：运行中的任务每 3 秒轮询更新状态
- 事件监听：通过 `cloud-agent-event` 实时接收任务完成/失败通知
- 支持取消运行中的任务
- 结果查看：展开查看已完成任务的输出摘要

**[Overlay.tsx](file:///d:/workspace/NeeCoder/src/components/Overlay.tsx)**
- Agent 提问浮层（AskQuestionOverlay）
- 危险操作确认浮层（ConfirmDangerDialog）

### 5.3 Tauri API 抽象层

**[useTauri.ts](file:///d:/workspace/NeeCoder/src/hooks/useTauri.ts)**（339 行）

统一的 API 封装层，核心设计：
- **`isTauri()`** — 检测是否在 Tauri 环境中运行
- **`tryInvoke()`** — 封装 invoke 调用，异常时返回 `null` 而非抛出
- **浏览器兼容** — 非 Tauri 环境返回 Mock 数据（开发调试用）
- **`listenToEvent()`** — 封装事件监听，返回 `UnlistenFn` 用于清理

导出 **30+ 个 API 函数**：文件操作（含 CRUD）、对话、补全、LSP、搜索、配置、会话管理、MCP 服务器管理、云 Agent 任务管理等

### 5.4 主题与样式系统

**[global.css](file:///d:/workspace/NeeCoder/src/styles/global.css)** — Catppuccin Mocha 暗色主题

**设计令牌：**
```css
:root {
  --bg-primary: #1e1e2e;     /* 主背景 */
  --bg-secondary: #181825;   /* 侧边栏/状态栏 */
  --bg-surface: #252536;     /* 卡片/悬浮层 */
  --bg-hover: #313244;       /* 交互悬停 */
  --text-primary: #cdd6f4;   /* 主要文本 */
  --text-secondary: #a6adc8; /* 次要文本 */
  --accent: #89b4fa;         /* 强调色 */
  --success: #a6e3a1;        /* 成功 */
  --warning: #f9e2af;        /* 警告 */
  --error: #f38ba8;          /* 错误 */
}
```

**技术方案：** 原生 CSS + BEM 命名 + CSS 变量，未使用 Tailwind 或 CSS-in-JS

---

## 6. 前后端通信协议

### invoke 命令（请求/响应）

前端通过 `invoke("command_name", { params })` 调用后端，后端返回 `Result<T, String>`。

### Tauri Events（推送）

后端通过 `app.emit("event-name", payload)` 推送到前端：

| 事件名 | 方向 | 用途 |
|--------|------|------|
| `chat-event` | 后端→前端 | 对话流式事件（14 种变体） |
| `completion-event` | 后端→前端 | 代码补全流式事件 |
| `cloud-agent-event` | 后端→前端 | 云 Agent 任务状态变更通知 |

### 状态同步机制

| 场景 | 机制 |
|------|------|
| 对话消息持久化 | 后端写入 Markdown 文件，前端通过 `get_session_messages` 加载 |
| 代码索引持久化 | 后端写入 SQLite，启动时自动加载 |
| 配置持久化 | 后端写入 JSON 文件，启动时自动加载 |
| Agent 状态 | 实时通过 Events 推送，不持久化 |

---

## 7. 数据流与关键路径

### 用户发送 Agent 消息

```
用户输入
  → ChatPanel.tsx: sendChatMessage()
    → invoke("send_message", { mode: "Agent" })
      → commands/chat.rs: send_message()
        → sanitize_messages() 净化历史
        → inject_memory_context() 注入记忆
        → #codebase 检测 → RAG 搜索注入上下文
        → tokio::spawn → agent::run_agent()
          → 循环: stream_chat → parse tool_calls → execute tools
            → needs_confirmation? → emit ConfirmRequest → 等待前端回答
            → execute_and_handle_special → post_execute_action
            → emit ToolCall/ToolResult/ToolRetry/TodoUpdate...
          → 循环结束
        → 持久化消息到 Session
        → append_note() 追加笔记
        → tokio::spawn → dreaming() LLM 摘要 → MEMORY.md
  ← Events: chat-event (Started → Delta... → ToolCall → ToolResult... → Finished)
  ← ChatPanel.tsx: 实时渲染消息流
```

### 代码补全请求

```
用户输入代码
  → CodeEditor.tsx: updateListener 检测变更
    → requestCompletion({ prefix, suffix, cursor })
      → invoke("request_completion", { context })
        → commands/completion.rs:
          → build_cache_key() → 检查 LRU 缓存
            → 命中: 直接 emit Finished
            → 未命中: build_fim_prompt() → stream_fim()
              → 逐 token emit Delta
              → 完成后 post_process_completion()
              → 存入缓存
              → emit Finished
  ← Events: completion-event (Started → Delta... → Finished)
  ← App.tsx: 累积 completionText
  ← CodeEditor.tsx: 渲染幽灵文本装饰器
  → 用户按 Tab → insertCompletion()
```

### 文件变更自动重索引

```
用户保存文件
  → OS 文件系统事件
    → notify crate 捕获
      → FileWatcher: poll_events() 防抖合并
        → lib.rs 后台循环 (每 2 秒)
          → 检查文件扩展名
          → tokio::spawn:
            → read_to_string() 读取文件
            → indexer.index_file() 重新分块 + embedding
            → indexer.save_to_db() 持久化到 SQLite
```

---

## 8. 安全机制

### 终端命令安全检查 ([run_terminal_command.rs](file:///d:/workspace/NeeCoder/src-tauri/src/agent/tools/run_terminal_command.rs))

`is_dangerous()` 函数拦截以下危险模式：
- `rm -rf /`、`rm -rf /*`
- `format`、`del /s`（Windows）
- `chmod -R 777`
- `curl | sh`、`wget | bash`（管道到 shell）
- `sudo` 提权命令

### 危险操作确认

3 种工具（`delete_file`、`delete_directory`、`run_terminal_command`）在执行前：
1. 后端发射 `ConfirmRequest` 事件
2. 前端弹出确认对话框
3. 用户点击 "Allow" → `invoke("answer_confirm")` → oneshot channel 通知 Agent 继续
4. 用户点击 "Deny" → Agent 收到 `[USER_DENIED]` 消息
5. 60 秒超时自动拒绝

### API Key 保护

- 运行时 `api_key` 使用 `#[serde(skip_serializing)]` 阻止序列化到磁盘
- 持久化使用 `api_key_encrypted`（XOR 混淆 + hex 编码）
- 启动时自动检测并迁移旧明文配置

### 文件树安全

`should_ignore()` 过滤 `.git`、`node_modules`、`target`、`dist` 等敏感/大型目录

---

## 9. 测试覆盖

共 **98 个测试**，分布在 4 个模块：

| 模块 | 测试数 | 覆盖范围 |
|------|--------|---------|
| Agent Tools | 20 | 工具注册、危险命令检测、工具执行 |
| RAG | 51 | 代码分块、BM25 搜索、向量搜索、混合搜索、SQLite 持久化 |
| Memory | 14 | 会话 CRUD、消息存取、长期记忆、笔记、搜索 |
| Completion | 13 | FIM prompt 构建、后处理、系统提示词 |

---

## 10. 依赖清单

### Rust 后端 (Cargo.toml)

| 依赖 | 版本 | 用途 |
|------|------|------|
| tauri | 2 | 桌面应用框架 |
| tauri-plugin-shell | 2 | Shell 插件 |
| tauri-plugin-dialog | 2 | 对话框插件 |
| tauri-plugin-fs | 2 | 文件系统插件 |
| tauri-plugin-process | 2 | 进程插件 |
| tauri-plugin-clipboard-manager | 2 | 剪贴板插件 |
| serde | 1 | 序列化/反序列化 |
| serde_json | 1 | JSON 序列化 |
| serde_yaml | 0.9 | YAML 序列化（会话存储） |
| tokio | 1 | 异步运行时 |
| reqwest | 0.12 | HTTP 客户端（LLM API） |
| futures-util | 0.3 | 流式处理辅助 |
| tokio-stream | 0.1 | Tokio 流 |
| notify | 7 | 文件系统监听 |
| regex | 1 | 正则表达式 |
| glob | 0.3 | 文件 glob 匹配 |
| rusqlite | 0.32 | SQLite（RAG 索引持久化） |
| uuid | 1 | UUID 生成 |
| chrono | 0.4 | 日期时间处理 |
| log | 0.4 | 日志 facade |
| anyhow | 1 | 错误处理 |
| thiserror | 2 | 自定义错误类型 |
| async-trait | 0.1 | 异步 trait 支持 |
| directories | 6 | 系统目录路径 |
| urlencoding | 2 | URL 编码 |

### 前端 (package.json)

| 依赖 | 用途 |
|------|------|
| react | UI 框架 |
| react-dom | DOM 渲染 |
| react-markdown | Markdown 渲染 |
| @codemirror/* | 代码编辑器 |
| @tauri-apps/api | Tauri 前端 API |
| @tauri-apps/plugin-* | Tauri 插件前端 API |
| react-syntax-highlighter | 代码语法高亮 |
| vite | 构建工具 |
| typescript | 类型检查 |
