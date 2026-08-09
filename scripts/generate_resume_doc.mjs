import {
  Document, Packer, Paragraph, TextRun, HeadingLevel,
  Table, TableRow, TableCell, WidthType, AlignmentType,
  BorderStyle, ShadingType, PageBreak,
} from "docx";
import * as fs from "fs";

const doc = new Document({
  styles: {
    default: {
      heading1: { run: { size: 32, bold: true, color: "1F4E79" } },
      heading2: { run: { size: 26, bold: true, color: "2E75B6" } },
      heading3: { run: { size: 22, bold: true, color: "3B8ED4" } },
    },
  },
  sections: [
    {
      children: [
        // ═══════════════════════════════════════════
        // 封面
        // ═══════════════════════════════════════════
        new Paragraph({ spacing: { before: 2400 } }),
        new Paragraph({
          alignment: AlignmentType.CENTER,
          children: [new TextRun({ text: "NeeCoder 系统架构与模块详解", size: 48, bold: true, color: "1F4E79" })],
        }),
        new Paragraph({
          alignment: AlignmentType.CENTER,
          spacing: { before: 200 },
          children: [new TextRun({ text: "—— Rust 高级开发工程师面试技术手册 ——", size: 24, color: "5A5A5A" })],
        }),
        new Paragraph({
          alignment: AlignmentType.CENTER,
          spacing: { before: 600 },
          children: [new TextRun({ text: "Tauri 2.0 + Rust (Edition 2024) + React 19 全栈 AI 编程助手", size: 22, color: "333333" })],
        }),
        new Paragraph({ spacing: { before: 2400 } }),
        new Paragraph({
          alignment: AlignmentType.CENTER,
          children: [new TextRun({ text: "2026年7月", size: 20, color: "888888" })],
        }),
        new Paragraph({ children: [], spacing: { before: 400 } }),
        new Paragraph({ children: [new PageBreak()] }),

        // ═══════════════════════════════════════════
        // 第1章：项目概述
        // ═══════════════════════════════════════════
        h1("一、项目概述"),
        p("NeeCoder 是一款基于 Tauri 2.0 + React 19 + Rust (Edition 2024) 构建的 AI 编程助手桌面应用。"
          + "项目实现了三模式对话（Ask/Edit/Agent）、AI Agent 自主工具执行、智能代码补全、RAG 代码搜索、"
          + "LSP 深度集成、MCP 协议桥接等核心功能，涵盖 50+ Tauri Command 和 30+ 内置工具。"),
        p("核心技术指标："),
        bullet("Rust 后端模块 15+，含 Agent、记忆、上下文、Checkpoint、RAG、MCP、LSP、Skill、沙箱等"),
        bullet("前端 React 19 组件 12+，含对话面板、代码编辑器、终端、文件浏览器、设置等"),
        bullet("多 LLM Provider 支持：OpenAI / Anthropic / Ollama / DeepSeek，统一接口抽象"),
        bullet("8 层安全防护：命令白名单、路径沙箱、Hook 拦截、Checkpoint 回滚、敏感数据脱敏等"),

        // ═══════════════════════════════════════════
        // 第2章：技术架构
        // ═══════════════════════════════════════════
        h1("二、技术架构总览"),
        h2("2.1 整体分层"),
        p("项目采用经典的前后端分离架构，通过 Tauri IPC 通信："),

        techTable([
          ["层级", "技术栈", "职责"],
          ["前端 UI", "React 19 + TypeScript + CodeMirror 6 + xterm.js", "IDE 界面、编辑器、终端、对话面板"],
          ["IPC 通信层", "Tauri 2.0 invoke / event 系统", "前后端类型安全的命令调用与事件推送"],
          ["Rust 后端", "tokio (async runtime) + serde + rusqlite + tiktoken-rs", "Agent 引擎、LLM 调度、文件操作、代码分析"],
          ["数据存储", "Markdown (会话) + JSONL (日志) + SQLite (RAG索引)", "轻量级文件存储 + 向量索引"],
        ]),

        h2("2.2 核心模块依赖关系"),
        p("Rust 后端模块按职责分为以下几个子系统："),
        bullet("Agent 引擎 (agent/) — 迭代循环、工具调度、子 Agent 编排、Loop Detector"),
        bullet("上下文管理 (agent/context.rs + agent/hooks.rs) — 消息压缩、Token 计数、运行时注入"),
        bullet("记忆系统 (memory/) — 6 个子系统：会话存储、长期记忆、Dreaming、偏好、日志、搜索"),
        bullet("Checkpoint (agent/checkpoint.rs) — Git 快照 + 精确回滚"),
        bullet("代码补全 (commands/completion.rs + llm/) — FIM 多候选补全"),
        bullet("RAG 搜索 (rag/) — 向量 + BM25 混合检索"),
        bullet("LLM (llm/) — 多 Provider 统一抽象"),
        bullet("LSP (lsp/) — 自实现 LSP 客户端"),
        bullet("MCP (mcp/) — Model Context Protocol 完整实现"),
        bullet("Skill (skill/) — 可扩展技能系统"),
        bullet("沙箱 (sandbox/) — 命令安全检查"),

        new Paragraph({ children: [new PageBreak()] }),

        // ═══════════════════════════════════════════
        // 第3章：Agent 系统
        // ═══════════════════════════════════════════
        h1("三、Agent 系统（核心引擎）"),
        h2("3.1 AgentInstance — 自主工具执行引擎"),

        p("Agent 核心类是 AgentInstance（agent/mod.rs，3400+ 行），封装完整的迭代循环、事件发射、"
          + "工具调度和上下文管理。所有字段为 owned 类型，支持 tokio::spawn 并行调度。"),

        h3("迭代循环核心流程"),
        codeBlock(
          "loop {\n"
          + "    1. compact_context_if_needed()    // 超预算 → LLM 压缩\n"
          + "    2. filter_tools_by_phase(i)        // 按阶段过滤工具集\n"
          + "    3. LLM 推理 (chat_with_tools)     // 返回 text 或 tool_calls\n"
          + "    4. 执行工具调用 (pre-hook → exec → post-hook)\n"
          + "    5. handle_tool_result()            // 预处理结果 + 截断\n"
          + "    6. 注入 post-batch 消息            // AutoDiagnose 等\n"
          + "    7. 检查停止条件 (达到 max_iterations / 取消 / 完成)\n"
          + "}"
        ),

        h3("执行阶段（ExecutionPhase）"),
        p("Agent 分三个阶段运行，每个阶段的工具集不同："),
        phaseTable([
          ["阶段", "工具策略", "典型行为"],
          ["Planning", "只读工具 (read/search/lsp)", "分析任务 → 输出结构化计划 → 等待用户审批"],
          ["Executing", "全部工具 (含写/删/终端)", "按计划逐步实现代码修改"],
          ["Done", "终端状态", "输出最终摘要"],
        ]),

        h3("4 个内置 Agent"),
        agentTable([
          ["Agent", "角色", "工具数", "特点"],
          ["Orchestrator", "主编排", "15", "任务分解 + 子 Agent 调度 + 依赖管理"],
          ["Code Writer", "代码编写", "13", "读写编辑 + 自动诊断 + UTF-8 安全截断"],
          ["Debugger", "调试诊断", "10", "根因分析 + 工具执行 + 只读优先"],
          ["Code Reviewer", "代码审查", "7", "P0~P3 分级 + 文件路径 + 修复建议"],
        ]),

        h3("Loop Detector — 死循环检测"),
        p("LoopDetector 跟踪最近 6 条工具调用的 (tool_name, arguments) 签名，检测重复模式："),
        bullet("完全重复的工具调用 → 注入警告提示"),
        bullet("同一文件连续编辑失败 → ErrorPatternHook 接管"),
        bullet("tool 调用结果中的编译/类型错误 → AutoDiagnoseHook 标记"),

        new Paragraph({ children: [new PageBreak()] }),

        h2("3.2 Tool 系统 — 30+ 内置工具"),
        p("工具注册采用 trait 统一接口，所有工具实现了 ToolExecutor trait："),
        codeBlock(
          "pub trait ToolExecutor: Send + Sync {\n"
          + "    fn name(&self) -> &str;\n"
          + "    fn description(&self) -> &str;\n"
          + "    fn parameters(&self) -> serde_json::Value;\n"
          + "    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<String>;\n"
          + "}"
        ),

        toolCategoryTable([
          ["类别", "工具", "典型用途"],
          ["文件操作", "read_file, write_file, edit, append_file, delete_file", "代码读写"],
          ["目录操作", "list_directory, create_directory, delete_directory", "项目结构管理"],
          ["代码搜索", "glob, grep, search_codebase, get_symbols", "代码定位"],
          ["Git 操作", "git_status, git_diff, git_log, git_commit, git_push, git_branch, git_checkout, git_stash, git_blame", "版本管理"],
          ["终端执行", "run_terminal_command", "构建/测试"],
          ["Web 搜索", "web_search, web_fetch", "在线文档"],
          ["任务管理", "todo_write, ask_user_question", "进度跟踪"],
          ["诊断", "get_diagnostics", "编译错误检查"],
          ["记忆", "memory_search", "长期记忆查询"],
          ["子 Agent", "dispatch_agent, dispatch_agents", "并行调度"],
        ]),

        h2("3.3 Sub-Agent 并行调度"),
        p("run_sub_agents_parallel 基于 DAG 依赖图实现分层并行执行："),
        bullet("无依赖任务：全部并发执行（tokio::task::JoinSet）"),
        bullet("有依赖任务：Topological Sort 分层执行，每层内并发"),
        bullet("循环依赖检测：remaining 任务无 ready 时 break + warn"),
        bullet("文件冲突检测：同一文件被多个 Agent 同时修改 → warn"),
        bullet("依赖输出注入：被依赖 Agent 的结果截断至 2000 字符注入下游"),

        new Paragraph({ children: [new PageBreak()] }),

        // ═══════════════════════════════════════════
        // 第4章：上下文管理系统
        // ═══════════════════════════════════════════
        h1("四、上下文管理系统"),
        p("上下文系统是 Agent 最核心的基础设施，由三层漏斗 + 七条注入管线构成。"),

        h2("4.1 System Prompt 装配层"),

        p("build_system_prompt() 按优先级分层叠加（agent/mod.rs L2591-2670）："),
        bullet("Layer 1: AGENT_SYSTEM_PROMPT / AgentDefinition.system_prompt（基础提示词）"),
        bullet("Layer 2: PROJECT_RULES.md（项目级规则，最高优先级）"),
        bullet("Layer 3: 自动语言检测提示（扫描文件扩展名，追加语言最佳实践）"),
        bullet("Layer 4: Cross-session Memory（长期记忆 top-20，艾宾浩斯筛选）"),
        bullet("Layer 5: User Preferences（文件类型分布、工具成功率统计）"),
        bullet("Layer 6: Custom Instructions（用户自定义指令，最高优先级）"),
        bullet("Layer 7: Execution Phase Hint（Planning/Executing 阶段提示）"),

        h2("4.2 Token 计数 — tiktoken-rs 精确计数"),
        p("使用 OnceLock 全局缓存 BPE Encoder，避免重复下载和初始化（agent/token_count.rs）："),
        codeBlock(
          "static BPE_ENCODER: OnceLock<CoreBPE> = OnceLock::new();\n"
          + "\n"
          + "fn get_bpe() -> &CoreBPE {\n"
          + "    BPE_ENCODER.get_or_init(|| {\n"
          + "        tiktoken_rs::o200k_base()  // GPT-4o tokenizer\n"
          + "            .unwrap_or_else(|| cl100k_base().unwrap())\n"
          + "    })\n"
          + "}\n"
          + "\n"
          + "pub fn estimate_total_tokens(messages, system_prompt, model) -> usize {\n"
          + "    system_tokens + Σ(message_tokens) + 3  // +3 for assistant reply primer\n"
          + "}"
        ),

        h2("4.3 Context Compaction — 智能消息压缩"),
        p("当消息总 token 超过 max_context_tokens 的 80% 时触发（agent/context.rs）："),

        h3("触发条件"),
        bullet("阈值：80% × max_context_tokens"),
        bullet("最小消息数：8 条"),
        bullet("保留首条用户消息（任务描述）"),
        bullet("保留最近 6 条消息（PRESERVE_RECENT = 6）"),

        h3("三明治压缩算法"),
        codeBlock(
          "[user:task] [assistant] [tool] ... [assistant] [tool] ... [assistant] [tool]\n"
          + "    ▲                                        ▲              ▲\n"
          + "  保留                                      压缩区           保留\n"
          + "  (任务描述)                               (LLM 摘要)       (最近6条)"
        ),

        h3("Tool-call 安全边界"),
        p("核心设计：如果切分点恰好落在 assistant(tool_calls) → tool 链中间，"
          + "会导致 LLM API 报 400 错误。算法向后遍历，找到 tool 消息的 assistant 父消息，"
          + "将整个人链纳入保留区。"),

        h3("Pre-compaction Memory Flush"),
        p("压缩前先从中段消息提取 [Lesson] 和 [Decision] 标签的关键知识，"
          + "作为额外上下文注入压缩 LLM，最多 8 条。"),

        h3("压缩参数"),
        bullet("源文本限制：≤ 12,000 字符"),
        bullet("单条消息截断：≤ 2,000 字符"),
        bullet("摘要 LLM：temperature=0.2, max_tokens=512"),
        bullet("输出格式：最多 12 条 bullet point，每句一行"),

        new Paragraph({ children: [new PageBreak()] }),

        h2("4.4 Hook 管线 — 7 个运行时拦截器"),
        p("HookManager 管理有序的 Hook 链，按注册顺序执行。每个 Hook 可在工具执行前/后/批量后注入消息。"),

        h3("注册顺序（即执行优先级）"),
        hookTable([
          ["阶段", "Hook", "职责", "类型"],
          ["Pre-tool", "SnapshotHook", "保存文件快照（撤销基础）", "不注入消息"],
          ["Pre-tool", "ConfirmHook", "危险操作弹窗确认 (60s超时自动拒绝)", "可能 Deny 阻止执行"],
          ["Post-tool", "SensitiveDataFilterHook", "正则脱敏 API key/密码/token (4种模式)", "可能修改结果"],
          ["Post-tool", "ErrorPatternHook", "同一文件连续3次失败 → 注入换策略提示", "可能注入消息"],
          ["Post-tool", "AuditLogHook", "JSONL 审计日志持久化", "不注入消息"],
          ["Post-tool", "OutputTruncateHook", "结果>8000字符 → 头2/3+尾1/3截断", "可能修改结果"],
          ["Post-tool", "FileChangeTrackerHook", "向前端推送文件变更事件", "不注入消息"],
          ["Post-batch", "AutoDiagnoseHook", "并行运行 cargo check/tsc/py_compile → 注入诊断", "可能注入消息"],
        ]),

        h3("AutoDiagnoseHook — 自动诊断 + 自修复闭环"),
        p("文件修改后，对所有受影响文件并行运行对应语言的编译检查："),
        bullet("Rust: cargo check --message-format=short"),
        bullet("TypeScript: npx tsc --noEmit"),
        bullet("Python: python -m py_compile"),
        bullet("Go: go vet"),
        bullet("所有诊断用 futures_util::future::join_all 全并行执行（非串行）"),
        bullet("结果以 [AUTO-DIAGNOSTICS] 前缀的 system 消息注入，让 LLM 自修复"),

        h3("ErrorPatternHook — 防止死循环"),
        p("跟踪最近 5 次工具调用，若同一文件连续 3 次失败，注入强制提示："),
        codeBlock('"[SYSTEM-HINT] You have failed on \'{file}\' 3 times consecutively.\n'
          + ' STOP retrying the same approach. Re-read the file first."'),

        new Paragraph({ children: [new PageBreak()] }),

        // ═══════════════════════════════════════════
        // 第5章：记忆系统
        // ═══════════════════════════════════════════
        h1("五、记忆系统（6 个子系统）"),
        p("记忆系统是层级化、带衰减策略的知识管理系统，不简单存文本，而是模拟人脑的遗忘-回忆机制。"),

        h2("5.1 艾宾浩斯遗忘曲线引擎（ebbinghaus.rs，685行）"),
        p("核心公式：R = e^(-t/S)，其中 t = 距上次回忆天数，S = 稳定性，R = 保留值 [0, 1]。"),
        p("每类知识有不同的 Stability 增长率："),

        ebbinghausTable([
          ["类别", "增长率", "归档阈值", "归档最小天数", "说明"],
          ["Core (永久)", "不衰减 (∞)", "0.0", "36,500", "永不遗忘，如用户偏好"],
          ["BugFix", "1.1 × ln(N+1)", "0.02", "90天", "精确事实，长期保留"],
          ["API Protocol", "1.1 × ln(N+1)", "0.02", "90天", "协议细节，长期保留"],
          ["Pattern", "ln(N+1)", "0.02", "60天", "设计模式，越回忆越牢固"],
          ["Decision", "ln(N+1)", "0.02", "60天", "架构决策，越回忆越牢固"],
          ["Coding", "ln(N+1)", "0.05", "30天", "一般编码知识"],
          ["Performance", "0.9 × ln(N+1)", "0.03", "45天", "性能优化"],
          ["Lesson", "0.8 × ln(N+1)", "0.05", "45天", "教训类适当快忘"],
          ["General", "固定 +0.2", "0.5", "7天", "非编码内容，主动遗忘"],
        ]),

        h3("双通道去重"),
        p("新增记忆时，用 Jaccard 词重叠（阈值 0.6）+ 话题关键词重叠（阈值 0.5）"
          + "双路检测相似条目，触发合并而非新增："),
        bullet("Channel 1: Jaccard word overlap — 词级别相似度"),
        bullet("Channel 2: Topic keyword overlap — 提取技术术语、文件路径做主题级相似度"),
        bullet("任一通道触发 → 合并：保留更高 stability、累加 recall_count、用更长的新文本"),

        h3("容量控制"),
        p("上限 50 条，超出时按 retention 从低到高淘汰。Core 类别 permanent（retention = f64::MAX）永不淘汰。"),

        new Paragraph({ children: [new PageBreak()] }),

        h2("5.2 六个子系统"),
        memoryTable([
          ["子系统", "存储介质", "核心功能"],
          ["Session Store", "Markdown 文件", "会话 CRUD + Context Window 滑动 + 分支 fork + 过期清理"],
          ["Long-term Memory", "MEMORY.md", "艾宾浩斯衰减 + 分类管理 + recall 更新 + 去重 + 容量控制"],
          ["Dreaming 管道", "(异步 LLM 调用)", "会话结束自动摘要 → 分级归档 → 过期清理 + 容量控制"],
          ["User Preferences", "JSON", "工具统计 + 文件类型分布 + 任务模式频率 → 注入 system prompt"],
          ["Agent Log", "JSONL 追加写", "每步 sync_data() 刷盘 → 崩溃恢复 + 会话回放"],
          ["Memory Search", "BM25", "全文搜索 MEMORY.md + daily notes → 命中触发 recall 更新"],
        ]),

        h2("5.3 Dreaming 管道（mod.rs L292-416）"),
        p("会话结束后 fire-and-forget 触发："),
        numbered("关键词检测（compute_coding_relevance）判断是否为编码会话 — 非编码直接跳过，不浪费 LLM 调用"),
        numbered("取前 20 条消息（每条截断 500 字符），构建结构化 prompt"),
        numbered("LLM 输出 [Goal]/[Decision]/[Lesson] 格式的结构化摘要"),
        numbered("全部摘要 → 追加到今日笔记；含 [Lesson]/[Decision] 标签的 → 写入 MEMORY.md"),
        numbered("结束时触发：过期归档（cleanup_expired）→ 容量控制（enforce_capacity，上限 50）"),

        new Paragraph({ children: [new PageBreak()] }),

        // ═══════════════════════════════════════════
        // 第6章：Checkpoint 系统
        // ═══════════════════════════════════════════
        h1("六、Checkpoint 系统 — Git 精确回滚"),
        p("基于 Git commit 实现的轻量级操作回滚机制（agent/checkpoint.rs，245行）。"),

        h2("6.1 工作流程"),
        codeBlock(
          "Agent 迭代开始\n"
          + "  │\n"
          + "  ├─ 确定即将修改的文件列表\n"
          + "  ├─ git add <files>                    ← 暂存\n"
          + "  ├─ git commit --allow-empty -m \"checkpoint: iteration X - editing main.rs\"\n"
          + "  │    └─ 提取 commit SHA\n"
          + "  ├─ 存入 CheckpointStore (session_id → Vec<Checkpoint>)\n"
          + "  │\n"
          + "  ▼ Agent 执行工具调用...\n"
          + "  \n"
          + "如果出问题:\n"
          + "  git checkout <hash> -- <files>        ← 精确回滚，仅受影响文件"
        ),

        h2("6.2 关键设计"),
        bullet("基于 Git 而非文件快照 — 利用 Git 对象存储，不自己存文件副本"),
        bullet("粒度和精确 — git checkout <hash> -- files，只恢复受影响文件，不污染整个仓库"),
        bullet("Tauri State 共享 — CheckpointStore 注册为 Tauri State，前后端均可查询/恢复"),
        bullet("超时保护 — 所有 Git 命令带 tokio::time::timeout（5-15秒）"),
        bullet("--allow-empty — 即使没有文件变更也允许创建 commit，保证每次迭代都有记录"),

        new Paragraph({ children: [new PageBreak()] }),

        // ═══════════════════════════════════════════
        // 第7章：代码补全系统
        // ═══════════════════════════════════════════
        h1("七、代码补全系统（FIM）"),
        p("基于 Fill-in-the-Middle 范式 + 多候选策略的智能代码补全。"),

        h2("7.1 多候选补全"),
        p("传统补全只生成一个结果。NeeCoder 通过不同 temperature 一次生成 3 个候选，"
          + "用户用 Alt+] 切换。"),

        codeBlock(
          "// 3 轮 FIM 请求，不同 temperature 生成差异化候选\n"
          + "let temperatures = [0.0, 0.3, 0.6];\n"
          + "for temp in temperatures {\n"
          + "    let request = FimRequest {\n"
          + "        prompt: <prefix>【hole】<suffix>,\n"
          + "        temperature: temp,\n"
          + "        max_tokens: 256,\n"
          + "    };\n"
          + "    candidates.push(llm.fim(request).await);\n"
          + "}\n"
          + "// 去重 → 存入 CompletionCandidates → 前端 Alt+] 切换"
        ),

        h3("多 Provider 统一抽象"),
        providerTable([
          ["Provider", "FIM 支持", "特点"],
          ["OpenAI", "gpt-4o 等", "原生 Chat API，FIM 通过 prompt 工程实现"],
          ["Anthropic", "Claude 系列", "原生 Chat API"],
          ["Ollama", "codellama/deepseek-coder", "本地模型，原生 FIM infill 支持"],
          ["DeepSeek", "deepseek-coder", "原生 FIM API，最优补全效果"],
        ]),

        new Paragraph({ children: [new PageBreak()] }),

        // ═══════════════════════════════════════════
        // 第8章：RAG 搜索
        // ═══════════════════════════════════════════
        h1("八、RAG 代码搜索系统"),
        p("混合检索策略：向量语义搜索（60%权重）+ BM25 关键词搜索（40%权重）。"),

        h2("8.1 索引管道"),
        bullet("文件监听（FileWatcher）→ 变更事件 → handle_file_change()"),
        bullet("异步读取文件 → 按扩展名过滤 → 分块 embedding"),
        bullet("向量存入 SQLite + 全文索引同步更新"),
        bullet("批次持久化到 DB"),

        h2("8.2 搜索流程"),
        bullet("Query 向量化 → ANN 搜索（余弦相似度）"),
        bullet("Query 分词 → BM25 排序"),
        bullet("结果融合：final_score = 0.6 × vector_score + 0.4 × bm25_score"),
        bullet("Top-N 结果返回，带文件路径和代码片段"),

        new Paragraph({ children: [new PageBreak()] }),

        // ═══════════════════════════════════════════
        // 第9章：MCP 协议
        // ═══════════════════════════════════════════
        h1("九、MCP 协议完整实现"),
        p("Model Context Protocol (MCP) 客户端完整实现，支持 JSON-RPC 通信、工具发现、服务管理。"),

        h2("9.1 架构"),
        bullet("McpClient — 进程管理 + stdin/stdout JSON-RPC 通信 + Initialize 握手"),
        bullet("McpRegistry — 全局注册表：管理 clients HashMap + tools HashMap"),
        bullet("McpToolWrapper — 将 MCP 工具转换为 OpenAI 兼容的 Function Call JSON"),
        bullet("McpToolBridge — Tauri 命令层，提供 connect/disconnect/list 接口"),

        h2("9.2 生命周期"),
        numbered("McpClient::spawn(config) — 启动子进程，建立 stdin/stdout 管道"),
        numbered("initialize() — 发送 initialize 请求，协商协议版本和能力"),
        numbered("list_tools() — 发送 tools/list，获取工具定义列表"),
        numbered("注册到 registry — 前缀工具名 (server_name__tool_name)，去重管理"),
        numbered("call_tool() — 通过 client.send_request() 调用工具"),
        numbered("disconnect() — 移除 client（Arc drop 触发 kill_on_drop 杀进程）+ 清理 tools"),
        numbered("tool_count_for_server() — 实时统计每个 server 的工具数"),

        new Paragraph({ children: [new PageBreak()] }),

        // ═══════════════════════════════════════════
        // 第10章：其他模块
        // ═══════════════════════════════════════════
        h1("十、其他核心模块"),

        h2("10.1 LSP 集成"),
        p("自实现 LSP 客户端（不依赖 VS Code 插件），支持："),
        bullet("goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol"),
        bullet("goToImplementation / prepareCallHierarchy / incomingCalls / outgoingCalls"),
        bullet("按语言自动选择 LSP Server，通过 stdin/stdout JSON-RPC 通信"),

        h2("10.2 LLM 统一抽象"),
        p("llm/mod.rs 提供统一的 LLM 接口，屏蔽 Provider 差异："),
        bullet("Chat：chat_with_tools() — 支持流式 + 非流式 + 工具调用"),
        bullet("FIM：stream_fim() — Fill-in-the-Middle 补全"),
        bullet("Provider 路由：OpenAI / Anthropic / Ollama / DeepSeek 统一 Request/Response 格式"),
        bullet("Streaming：通过 callback 回调 token 流，支持取消"),

        h2("10.3 Skill 系统"),
        p("可扩展的技能系统（skill/mod.rs + skill/builtin.rs）："),
        bullet("每个 Skill 定义独立的 system_prompt + tool_names + mode"),
        bullet("内置 Skill：auto-review, plan-mode 等"),
        bullet("支持用户自定义 Skill 配置文件"),

        h2("10.4 沙箱安全（8层防护）"),
        p("sandbox/mod.rs 实现多层命令安全过滤："),
        bullet("Layer 1: 禁止 rm/format/disable/sudo 等危险命令"),
        bullet("Layer 2: ConfirmHook — delete/terminal 操作弹窗确认"),
        bullet("Layer 3: Checkpoint — 每次写入前自动 git commit"),
        bullet("Layer 4: SnapshotHook — 内存级文件快照"),
        bullet("Layer 5: SensitiveDataFilterHook — 结果中脱敏 API key/密码"),
        bullet("Layer 6: 文件路径白名单 — 操作限制在项目目录内"),
        bullet("Layer 7: 进程隔离 — 工具调用在子进程执行"),
        bullet("Layer 8: 超时保护 — 所有外部命令带 timeout"),

        h2("10.5 PTY 终端"),
        p("基于 portable-pty 的原生伪终端实现（非简单 process spawn）："),
        bullet("全双工 stdin/stdout 通信 + 终端 resize 支持"),
        bullet("ANSI 转义序列解析 + xterm.js 前端渲染"),
        bullet("跨平台兼容（Windows/macOS/Linux）"),

        h2("10.6 遥测系统"),
        p("telemetry/mod.rs — 匿名使用统计，用于产品优化："),
        bullet("Session 开始/结束事件（耗时、token 用量、成功/失败）"),
        bullet("工具调用频率和成功率"),
        bullet("LLM Provider 使用分布"),

        new Paragraph({ children: [new PageBreak()] }),

        // ═══════════════════════════════════════════
        // 第11章：面试要点
        // ═══════════════════════════════════════════
        h1("十一、面试核心要点总结"),

        h2("11.1 Rust 技术亮点"),
        rustHighlightTable([
          ["技术点", "实现细节", "面试价值"],
          ["Async Runtime", "tokio + tokio::sync (RwLock, Mutex, oneshot, mpsc)", "全异步架构设计"],
          ["并发安全", "Arc<Mutex<T>> / Arc<RwLock<T>> / OnceLock / AtomicBool", "零 data race 保障"],
          ["Trait 系统", "ToolExecutor, LifecycleHook, SessionStore, MemoryBackend", "可扩展接口设计"],
          ["内存管理", "owned 类型设计避免生命周期参数 + fire-and-forget spawn", "支持并行调度"],
          ["错误处理", "Result<T, String> + ? 传播 + 分层日志", "全链路错误追踪"],
          ["序列化", "serde + serde_json + serde_yaml + serde(flatten/tag)", "多格式互操作"],
          ["文件 I/O", "tokio::fs + sync_data() 刷盘 + char-aware 安全截断", "数据一致性保障"],
          ["进程管理", "Command + stdin/stdout pipe + kill_on_drop", "子进程全生命周期管理"],
        ]),

        h2("11.2 架构设计亮点"),
        architectureHighlightTable([
          ["亮点", "说明"],
          ["三层漏斗上下文", "System Prompt 装配 → 消息流管理 → LLM 压缩，每层独立可测试"],
          ["Hook 管线", "7 个 Hook 按序执行，pre/post/batch 三阶段，支持 Deny/Modify/Inject"],
          ["艾宾浩斯衰减", "6 种知识类别 × 差异化衰减率 × 双通道去重，模拟人类记忆机制"],
          ["Git Checkpoint", "基于 Git commit 的精确回滚，非文件快照方案，轻量且精确"],
          ["多候选 FIM", "不同 temperature 并行生成 3 个候选，去重后提供 Alt+] 切换"],
          ["Sub-Agent DAG", "依赖图 → Topological Sort → 层级并行，支持循环检测和文件冲突检测"],
          ["MCP 完整实现", "JSON-RPC + 进程管理 + 工具注册 + 实时统计，不依赖第三方 SDK"],
          ["8 层安全", "命令白名单 → 确认拦截 → Checkpoint → 快照 → 脱敏 → 路径限制 → 隔离 → 超时"],
        ]),

        h2("11.3 数据流全景"),
        p("从用户输入到 Agent 完成任务的完整数据流："),
        codeBlock(
          "用户输入 (React UI)\n"
          + "  │\n"
          + "  ▼ Tauri invoke('run_agent_session', { session_id, message })\n"
          + "  │\n"
          + "  ├─ 创建 AgentInstance\n"
          + "  ├─ build_system_prompt()         ← 7 层装配\n"
          + "  ├─ inject_memory_context()       ← 艾宾浩斯筛选\n"
          + "  │\n"
          + "  ▼ 迭代循环 (最多 N 次)\n"
          + "  │  ├─ compact_context_if_needed()  ← Token 超 80% 触发压缩\n"
          + "  │  ├─ filter_tools_by_phase()     ← 按阶段过滤\n"
          + "  │  ├─ LLM 推理 (chat_with_tools)\n"
          + "  │  ├─ pre-tool hooks              ← Snapshot + Confirm\n"
          + "  │  ├─ 工具执行\n"
          + "  │  ├─ post-tool hooks             ← Filter + Error + Audit + Truncate\n"
          + "  │  └─ post-batch hooks            ← AutoDiagnose\n"
          + "  │\n"
          + "  ▼ 完成\n"
          + "  ├─ event::AgentComplete → 前端展示结果\n"
          + "  ├─ dreaming() → 记忆固化\n"
          + "  └─ telemetry → 使用统计"
        ),

        new Paragraph({ spacing: { before: 600 } }),
        p("—— 全文完 ——", { align: "center", bold: true, size: 20 }),

      ],
    },
  ],
});

// ═══════════════════════════════════════════
// Helper functions
// ═══════════════════════════════════════════
function h1(text) {
  return new Paragraph({ heading: HeadingLevel.HEADING_1, children: [new TextRun(text)], spacing: { before: 400, after: 200 } });
}
function h2(text) {
  return new Paragraph({ heading: HeadingLevel.HEADING_2, children: [new TextRun(text)], spacing: { before: 300, after: 150 } });
}
function h3(text) {
  return new Paragraph({ heading: HeadingLevel.HEADING_3, children: [new TextRun(text)], spacing: { before: 200, after: 100 } });
}
function p(text, opts = {}) {
  return new Paragraph({
    spacing: { before: 80, after: 80 },
    alignment: opts.align === "center" ? AlignmentType.CENTER : AlignmentType.LEFT,
    children: [new TextRun({ text, size: opts.size || 21, bold: opts.bold || false })],
  });
}
function bullet(text) {
  return new Paragraph({
    spacing: { before: 40, after: 40 },
    bullet: { level: 0 },
    children: [new TextRun({ text, size: 21 })],
  });
}
function numbered(text) {
  return new Paragraph({
    spacing: { before: 40, after: 40 },
    numbering: { reference: "default-numbering", level: 0 },
    children: [new TextRun({ text, size: 21 })],
  });
}
function codeBlock(text) {
  return new Paragraph({
    spacing: { before: 80, after: 80 },
    shading: { type: ShadingType.SOLID, color: "F5F5F5" },
    border: { left: { style: BorderStyle.SINGLE, size: 4, color: "CCCCCC" } },
    indent: { left: 200 },
    children: [new TextRun({ text, size: 18, font: "Consolas" })],
  });
}

function createTable(headers, rows) {
  return new Table({
    width: { size: 100, type: WidthType.PERCENTAGE },
    rows: [
      new TableRow({
        tableHeader: true,
        children: headers.map(h => new TableCell({
          shading: { type: ShadingType.SOLID, color: "2E75B6" },
          children: [new Paragraph({ children: [new TextRun({ text: h, bold: true, size: 20, color: "FFFFFF" })] })],
        })),
      }),
      ...rows.map(row =>
        new TableRow({
          children: row.map(cell =>
            new TableCell({
              children: [new Paragraph({ children: [new TextRun({ text: cell, size: 20 })] })],
            })
          ),
        })
      ),
    ],
  });
}

function techTable(rows) { return createTable(rows[0], rows.slice(1)); }
function phaseTable(rows) { return createTable(rows[0], rows.slice(1)); }
function agentTable(rows) { return createTable(rows[0], rows.slice(1)); }
function toolCategoryTable(rows) { return createTable(rows[0], rows.slice(1)); }
function hookTable(rows) { return createTable(rows[0], rows.slice(1)); }
function ebbinghausTable(rows) { return createTable(rows[0], rows.slice(1)); }
function memoryTable(rows) { return createTable(rows[0], rows.slice(1)); }
function providerTable(rows) { return createTable(rows[0], rows.slice(1)); }
function rustHighlightTable(rows) { return createTable(rows[0], rows.slice(1)); }
function architectureHighlightTable(rows) { return createTable(rows[0], rows.slice(1)); }

// ── Generate ──
const buffer = await Packer.toBuffer(doc);
fs.writeFileSync("F:\\简历\\NeeCoder系统模块详解_面试手册.docx", buffer);
console.log("✅ 文档已生成: F:\\简历\\NeeCoder系统模块详解_面试手册.docx");
