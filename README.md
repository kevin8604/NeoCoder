# NeoCoder

**AI-Powered Coding Assistant Desktop Application**
**AI 驱动的编程助手桌面应用**

[English](#english) | [中文](#中文)

---

## English

NeoCoder is a desktop AI coding assistant built with **Tauri 2.0 + React + Rust**. It combines a full-featured code editor with an AI chat panel, intelligent code completion, RAG-based code search, multi-agent orchestration, and LSP integration.

### Features

- **Three Chat Modes** — Ask (Q&A), Edit (code suggestions with diff preview), Agent (autonomous tool execution)
- **AI Agent System** — Iterative reasoning loop with 41 built-in tools, sub-agent orchestration, and dangerous operation confirmation
- **Intelligent Code Completion** — FIM (Fill-in-the-Middle) paradigm with LRU caching and ghost text display
- **RAG Code Search** — Hybrid search combining vector similarity (60%) and BM25 keyword matching (40%)
- **Multi-Provider LLM** — Supports OpenAI, DeepSeek, Anthropic, and Ollama (local)
- **LSP Integration** — 10 language servers (Rust, TypeScript, Python, Go, Java, C/C++, Ruby, PHP, C#, Kotlin)
- **Memory System** — Session persistence (Markdown), long-term memory (MEMORY.md), daily notes, and dreaming (LLM-generated summaries)
- **MCP Protocol** — Connect to external MCP servers for extended tool capabilities
- **Cloud Agent** — Background task execution with status tracking and cancellation
- **Browser Automation** — Persistent CDP session (navigate / click / type / screenshot / text extraction) for UI verification
- **Browser Recording & Replay** — `web_browser` logs every successful action; `export_script` / `replay` re-runs a UI flow; `screenshot_diff` does pixel-level visual regression with a red-highlighted diff image
- **Test Failure Locating** — `run_tests` extracts failing `path:line` locations from compiler/test output and attaches the surrounding source lines
- **Test-Driven Loop** — `generate_tests` + line-coverage guidance (`coverage`) + TDD state machine, closed with `run_tests` / `run_build` verification
- **Error Self-Healing** — `auto_fix` diagnoses failed commands (root cause + fix steps + retry command), driving the fail → diagnose → fix → retry loop
- **Agent Timeline** — Real-time event stream panel (tool calls, thinking, checkpoints, retries) for execution visibility
- **Multi-Workspace Runtime** — Independent workspaces with per-project code index, watchers and skills; switch via a picker in the status bar (list / rename / remove)
- **Inline Chat** — Multi-turn conversational editing: Ctrl+K on a selection, then keep refining the result with follow-up instructions before accepting
- **Symbol Mentions** — `@symbolName` in chat resolves document symbols via LSP and attaches the file at the definition line
- **Line-Level Completion** — Tab accepts the whole ghost text; Ctrl/Cmd+Right accepts only the current line
- **Security** — Dangerous command interception, operation confirmation dialogs, API key encryption

### Tech Stack

| Layer | Technology |
|-------|-----------|
| Desktop Framework | Tauri 2.0 |
| Backend | Rust (Edition 2024) + Tokio |
| Frontend | React 19 + TypeScript |
| Editor | CodeMirror 6 |
| Build Tool | Vite |
| Database | SQLite (rusqlite) |
| HTTP | reqwest (SSE streaming) |
| Theme | Catppuccin Mocha |

### Prerequisites

- [Node.js](https://nodejs.org/) >= 18
- [Rust](https://www.rust-lang.org/tools/install) >= 1.85 (Edition 2024)
- [Tauri 2.0 prerequisites](https://v2.tauri.app/start/prerequisites/)

### Getting Started

```bash
# Clone the repository
git clone https://github.com/your-username/NeoCoder.git
cd NeoCoder

# Install frontend dependencies
npm install

# Run in development mode
npm run tauri dev

# Build for production
npm run tauri build
```

### Project Structure

```
NeoCoder/
├── src/                          # Frontend (React + TypeScript)
│   ├── components/               # UI components
│   │   ├── ChatPanel.tsx         # AI chat panel (Ask/Edit/Agent)
│   │   ├── CodeEditor.tsx        # CodeMirror 6 editor
│   │   ├── FileExplorer.tsx      # File tree browser
│   │   ├── SearchPanel.tsx       # Code search
│   │   ├── Settings.tsx          # Settings & MCP config
│   │   ├── CloudAgentPanel.tsx   # Cloud agent tasks
│   │   └── ...
│   ├── hooks/useTauri.ts         # Tauri API abstraction layer
│   └── styles/global.css         # Catppuccin Mocha theme
├── src-tauri/                    # Backend (Rust)
│   ├── src/
│   │   ├── agent/                # Agent system + 19 tools
│   │   ├── chat/                 # Chat message models
│   │   ├── commands/             # Tauri commands (40+ APIs)
│   │   ├── completion/           # FIM code completion
│   │   ├── config/               # Settings & API key encryption
│   │   ├── llm/                  # Multi-provider LLM communication
│   │   ├── lsp/                  # LSP protocol implementation
│   │   ├── memory/               # Session/long-term/dreaming memory
│   │   ├── rag/                  # RAG code indexing & hybrid search
│   │   ├── mcp/                  # MCP client & tool bridge
│   │   └── ...
│   └── tools.json                # Agent tool schemas
└── ARCHITECTURE.md               # Detailed architecture docs
```

### Agent Tools

| Tool | Description | Confirmation |
|------|-------------|:---:|
| `read_file` | Read file contents | |
| `write_file` | Create/overwrite files | |
| `append_file` | Append content to file | |
| `edit` | Precise string replacement | |
| `delete_file` | Delete a file | Yes |
| `list_directory` | List directory contents | |
| `create_directory` | Create directories | |
| `delete_directory` | Recursively delete directories | Yes |
| `search_codebase` | RAG semantic code search | |
| `grep` | Text pattern search | |
| `glob` | Glob pattern file matching | |
| `get_symbols` | Extract symbol definitions | |
| `get_diagnostics` | Compiler/linter diagnostics | |
| `memory_search` | Memory system semantic search | |
| `run_terminal_command` | Execute shell commands | Yes |
| `run_tests` | Detect project type & run tests (read-only) | |
| `run_build` | Detect project type & run build (read-only) | |
| `run_terminal_session` | Persistent PTY shell session | |
| `git_status` / `git_diff` / `git_log` / `git_blame` | Inspect repo state & history | |
| `git_commit` | Stage & commit changes (`auto_summary` derives message from diff) | |
| `git_branch` / `git_checkout` / `git_stash` / `git_push` | Branch, checkout, stash, push | |
| `web_search` | Web search | |
| `web_fetch` | Fetch web page content | |
| `web_preview` | Screenshot a running web app (headless) | |
| `web_browser` | Persistent CDP browser automation (navigate/click/type/screenshot) + recording / replay / screenshot diff | |
| `generate_tests` | Generate unit/integration tests via LLM & run them | |
| `coverage` | Line-coverage guidance (`cargo llvm-cov` uncovered lines) | |
| `tdd` | TDD state machine (start/stop/inspect) | |
| `auto_fix` | Diagnose a failed command → root cause + fix steps + retry | |
| `generate_diagram` | Generate Mermaid diagrams | |
| `todo_write` | Task list management | |
| `ask_user_question` | Ask user during execution | |
| `dispatch_agent` / `dispatch_agents` | Spawn sub-agents (serial/parallel) | |
| `a2a_invoke` | Invoke remote A2A agents | |

### Built-in Agents

| Agent | Role | Max Iterations |
|-------|------|:-:|
| `orchestrator` | Main agent / dispatcher | 15 |
| `code_writer` | Code writing specialist | 10 |
| `debugger` | Debug specialist | 8 |
| `reviewer` | Code review (read-only) | 5 |

### License

MIT

---

## 中文

NeoCoder 是一款基于 **Tauri 2.0 + React + Rust** 构建的桌面 AI 编程助手。它将全功能代码编辑器与 AI 对话面板、智能代码补全、RAG 代码搜索、多 Agent 协作和 LSP 语言服务器集成于一体。

### 核心功能

- **三种对话模式** — Ask（问答）、Edit（带 diff 预览的代码建议）、Agent（自主工具执行）
- **AI Agent 系统** — 迭代推理循环，41 个内置工具，子 Agent 调度，危险操作确认
- **智能代码补全** — FIM（Fill-in-the-Middle）范式，LRU 缓存，幽灵文本展示
- **RAG 代码搜索** — 混合搜索：向量相似度（60%）+ BM25 关键词匹配（40%）
- **多 LLM 提供商** — 支持 OpenAI、DeepSeek、Anthropic 和 Ollama（本地部署）
- **LSP 集成** — 10 种语言服务器（Rust、TypeScript、Python、Go、Java、C/C++、Ruby、PHP、C#、Kotlin）
- **记忆系统** — 会话持久化（Markdown）、长期记忆（MEMORY.md）、每日笔记、Dreaming（LLM 摘要生成）
- **MCP 协议** — 连接外部 MCP 服务器以扩展工具能力
- **云 Agent** — 后台任务执行，状态跟踪与取消支持
- **浏览器自动化** — 持久化 CDP 会话（导航/点击/输入/截图/文本提取），用于 UI 验证
- **浏览器录制回放** — `web_browser` 记录每次成功动作，`export_script`/`replay` 重新执行 UI 流程；`screenshot_diff` 做像素级视觉回归（diff 图红色高亮变更像素）
- **测试失败定位** — `run_tests` 从编译/测试输出中提取失败位置 `path:line` 并附上周围源码行
- **测试驱动闭环** — `generate_tests` + 行覆盖率引导（`coverage`）+ TDD 状态机，由 `run_tests` / `run_build` 验证闭环
- **错误自愈** — `auto_fix` 诊断失败命令（根因 + 修复步骤 + 重试命令），驱动"失败→诊断→修复→重试"循环
- **Agent 执行时间线** — 实时事件流面板（工具调用、思考、检查点、重试），执行过程全程可见
- **多工作区运行时** — 每个工作区独立代码索引/Watcher/Skills，状态栏选择器切换（列表/重命名/删除）
- **Inline Chat** — 多轮对话式编辑：Ctrl+K 选中代码后，可反复追加指令优化结果再接受
- **符号级 @ 引用** — 输入 `@符号名` 时通过 LSP 解析当前文件符号，按定义行附加文件
- **行级补全接受** — Tab 接受整段幽灵文本，Ctrl/Cmd+Right 仅接受当前行
- **安全机制** — 危险命令拦截、操作确认对话框、API Key 加密存储

### 技术栈

| 层级 | 技术 |
|------|------|
| 桌面框架 | Tauri 2.0 |
| 后端 | Rust (Edition 2024) + Tokio |
| 前端 | React 19 + TypeScript |
| 编辑器 | CodeMirror 6 |
| 构建工具 | Vite |
| 数据库 | SQLite (rusqlite) |
| HTTP | reqwest (SSE 流式) |
| 主题 | Catppuccin Mocha |

### 环境要求

- [Node.js](https://nodejs.org/) >= 18
- [Rust](https://www.rust-lang.org/tools/install) >= 1.85 (Edition 2024)
- [Tauri 2.0 前置条件](https://v2.tauri.app/start/prerequisites/)

### 快速开始

```bash
# 克隆仓库
git clone https://github.com/your-username/NeoCoder.git
cd NeoCoder

# 安装前端依赖
npm install

# 开发模式运行
npm run tauri dev

# 生产环境构建
npm run tauri build
```

### 项目结构

```
NeoCoder/
├── src/                          # 前端 (React + TypeScript)
│   ├── components/               # UI 组件
│   │   ├── ChatPanel.tsx         # AI 对话面板 (Ask/Edit/Agent)
│   │   ├── CodeEditor.tsx        # CodeMirror 6 编辑器
│   │   ├── FileExplorer.tsx      # 文件树浏览器
│   │   ├── SearchPanel.tsx       # 代码搜索
│   │   ├── Settings.tsx          # 设置 & MCP 配置
│   │   ├── CloudAgentPanel.tsx   # 云 Agent 任务
│   │   └── ...
│   ├── hooks/useTauri.ts         # Tauri API 抽象层
│   └── styles/global.css         # Catppuccin Mocha 主题
├── src-tauri/                    # 后端 (Rust)
│   ├── src/
│   │   ├── agent/                # Agent 系统 + 19 个工具
│   │   ├── chat/                 # 对话消息模型
│   │   ├── commands/             # Tauri 命令 (40+ API)
│   │   ├── completion/           # FIM 代码补全
│   │   ├── config/               # 设置 & API Key 加密
│   │   ├── llm/                  # 多提供商 LLM 通信
│   │   ├── lsp/                  # LSP 协议实现
│   │   ├── memory/               # 会话/长期/Dreaming 记忆
│   │   ├── rag/                  # RAG 代码索引 & 混合搜索
│   │   ├── mcp/                  # MCP 客户端 & 工具桥接
│   │   └── ...
│   └── tools.json                # Agent 工具 Schema 定义
└── ARCHITECTURE.md               # 详细架构文档
```

### Agent 工具集

| 工具 | 功能 | 需要确认 |
|------|------|:---:|
| `read_file` | 读取文件内容 | |
| `write_file` | 创建/覆写文件 | |
| `append_file` | 追加内容到文件 | |
| `edit` | 精确字符串替换 | |
| `delete_file` | 删除文件 | 是 |
| `list_directory` | 列出目录内容 | |
| `create_directory` | 创建目录 | |
| `delete_directory` | 递归删除目录 | 是 |
| `search_codebase` | RAG 语义代码搜索 | |
| `grep` | 文本模式搜索 | |
| `glob` | Glob 模式文件匹配 | |
| `get_symbols` | 提取符号定义 | |
| `get_diagnostics` | 编译器/linter 诊断 | |
| `memory_search` | 记忆系统语义搜索 | |
| `run_terminal_command` | 执行终端命令 | 是 |
| `run_tests` | 识别项目类型并运行测试（只读） | |
| `run_build` | 识别项目类型并运行构建（只读） | |
| `run_terminal_session` | 持久化 PTY shell 会话 | |
| `git_status` / `git_diff` / `git_log` / `git_blame` | 查看仓库状态与历史 | |
| `git_commit` | 提交变更（`auto_summary` 从 diff 推导消息） | |
| `git_branch` / `git_checkout` / `git_stash` / `git_push` | 分支、切换、暂存、推送 | |
| `web_search` | 网页搜索 | |
| `web_fetch` | 获取网页内容 | |
| `web_preview` | 截取运行中 Web 应用截图（无头浏览器） | |
| `web_browser` | 持久化 CDP 浏览器自动化（导航/点击/输入/截图） | |
| `generate_tests` | LLM 生成单元/集成测试并运行 | |
| `coverage` | 行覆盖率引导（`cargo llvm-cov` 未覆盖行） | |
| `tdd` | TDD 状态机（启动/停止/查看） | |
| `auto_fix` | 诊断失败命令 → 根因 + 修复步骤 + 重试 | |
| `generate_diagram` | 生成 Mermaid 图表 | |
| `todo_write` | 任务列表管理 | |
| `ask_user_question` | 执行中向用户提问 | |
| `dispatch_agent` / `dispatch_agents` | 调度子 Agent（串行/并行） | |
| `a2a_invoke` | 调用远程 A2A Agent | |

### 内置 Agent

| Agent | 角色 | 最大迭代次数 |
|-------|------|:-:|
| `orchestrator` | 主 Agent / 调度器 | 15 |
| `code_writer` | 代码编写专家 | 10 |
| `debugger` | 调试专家 | 8 |
| `reviewer` | 代码审查（只读） | 5 |

### 许可证

MIT
