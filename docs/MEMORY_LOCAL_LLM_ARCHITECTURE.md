# NeeCoder 记忆系统 × 本地大模型融合架构设计

> 将三层记忆系统与本地大模型深度集成，构建"使用 → 记忆 → 微调 → 进化"的自闭环系统。

---

## 目录

1. [设计目标](#1-设计目标)
2. [现状分析](#2-现状分析)
3. [整体架构](#3-整体架构)
4. [三层记忆模型](#4-三层记忆模型)
5. [本地大模型集成](#5-本地大模型集成)
6. [Dreaming 管道](#6-dreaming-管道)
7. [微调数据管道](#7-微调数据管道)
8. [LoRA 微调系统](#8-lora-微调系统)
9. [配置设计](#9-配置设计)
10. [数据流与状态机](#10-数据流与状态机)
11. [前端交互设计](#11-前端交互设计)
12. [实施计划](#12-实施计划)
13. [技术挑战与对策](#13-技术挑战与对策)
14. [评估体系](#14-评估体系)

---

## 1. 设计目标

### 1.1 核心愿景

```
用户使用越多 → 记忆积累越多 → 微调数据越丰富 → 本地模型越懂用户 → 正向循环
```

### 1.2 具体目标

| 目标 | 衡量标准 |
|------|---------|
| **隐私优先** | 记忆数据和微调过程全部在本地完成，不依赖云端 |
| **成本可控** | Dreaming 和简单推理走本地模型，零 API 费用 |
| **个性化** | 通过 LoRA 微调，模型适配用户的代码库和编程习惯 |
| **渐进增强** | 从现有架构平滑演进，不破坏已有功能 |
| **可观测** | 记忆状态、微调进度、模型质量对用户透明可见 |

### 1.3 设计原则

- **分层解耦**：记忆层、推理层、微调层各自独立，可单独替换
- **优雅降级**：本地模型不可用时自动 fallback 到远程 API
- **数据自治**：用户拥有完整的数据主权（查看、编辑、删除、导出）
- **资源感知**：根据硬件条件（GPU/CPU/RAM）自动选择最优策略

---

## 2. 现状分析

### 2.1 当前记忆系统

```
MemoryManager
├── SessionStorage      → sessions/<uuid>/messages/*.md  （会话消息）
├── LongTermMemory      → MEMORY.md                        （长期记忆）
├── DailyNotes           → notes/YYYY-MM-DD.md             （每日笔记）
├── MemorySearch         → 关键词 grep 搜索                 （纯文本匹配）
└── tools.rs             → Agent 可调用的读/写/搜索工具
```

### 2.2 现状不足

| 问题 | 影响 |
|------|------|
| Dreaming 依赖远程 API | 每次 Agent 完成都消耗 token，隐私数据上传云端 |
| MemorySearch 纯关键词 | 搜"性能优化"找不到"performance improvement" |
| 无微调能力 | 模型无法从历史交互中学习用户习惯 |
| LongTermMemory 全文读写 | 文件越大效率越低，并发不安全 |
| 无记忆清理机制 | MEMORY.md 无限增长，注入 system prompt 膨胀 |
| 训练数据未结构化 | 每日笔记是自由文本，无法直接用于微调 |

### 2.3 现有可复用资产

| 资产 | 位置 | 复用方式 |
|------|------|---------|
| LlmProvider::Ollama | `config/mod.rs` | 本地模型已有 Provider 枚举支持 |
| stream_chat 函数 | `llm/mod.rs` | 本地模型走相同的流式接口 |
| dreaming 函数 | `memory/mod.rs` | 改造为支持本地模型 |
| MEMORY.md 格式 | `memory/long_term.rs` | 微调数据源 |
| notes/ 目录 | `memory/notes.rs` | 微调数据源 |
| Tauri State 管理 | `lib.rs` | 新增 LocalModelState |

---

## 3. 整体架构

```
┌─────────────────────────────────────────────────────────────────┐
│                      用户交互层（Tauri 前端）                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────┐    │
│  │ ChatPanel │  │CodeEditor│  │ Settings │  │ Memory Panel │    │
│  │           │  │          │  │          │  │ (新增)        │    │
│  └─────┬────┘  └────┬─────┘  └────┬─────┘  └──────┬───────┘    │
└────────┼────────────┼────────────┼─────────────────┼────────────┘
         │            │            │                 │
┌────────┼────────────┼────────────┼─────────────────┼────────────┐
│        ▼            ▼            ▼                 ▼            │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                    Tauri Command Layer                    │  │
│  │  send_message │ request_completion │ trigger_finetune   │  │
│  │  get_memory_status │ export_training_data │ ...          │  │
│  └────────────────────────┬─────────────────────────────────┘  │
│                           │                                    │
│  ┌────────────────────────▼─────────────────────────────────┐  │
│  │                    Agent 主循环                           │  │
│  │  ┌─────────────┐  ┌──────────────┐  ┌────────────────┐  │  │
│  │  │ Tool System  │  │ Hook System  │  │ Context Manager│  │  │
│  │  └─────────────┘  └──────────────┘  └────────────────┘  │  │
│  └────────────────────────┬─────────────────────────────────┘  │
│                           │                                    │
│  ┌────────────────────────▼─────────────────────────────────┐  │
│  │              LLM 路由层 (新增)                            │  │
│  │                                                          │  │
│  │  ┌─────────────┐    ┌──────────────┐    ┌────────────┐  │  │
│  │  │ 远程 LLM     │    │ 本地推理 LLM  │    │ 本地微调   │  │  │
│  │  │ (DeepSeek)  │    │ (Ollama)     │    │ LoRA       │  │  │
│  │  │ Agent 主循环  │    │ Dreaming    │    │ 离线训练    │  │  │
│  │  │ 高质量推理   │    │ 简单问答     │    │            │  │  │
│  │  └─────────────┘    └──────────────┘    └────────────┘  │  │
│  └────────────────────────┬─────────────────────────────────┘  │
│                           │                                    │
│  ┌────────────────────────▼─────────────────────────────────┐  │
│  │              三层记忆系统                                  │  │
│  │                                                          │  │
│  │  Layer 1 (短期)     Layer 2 (中期)      Layer 3 (长期)   │  │
│  │  Session Messages   Daily Notes         MEMORY.md       │  │
│  │  *.md (原始对话)    YYYY-MM-DD.md       结构化知识库     │  │
│  │       │                  │                    │        │  │
│  │       └──→ Dreaming ──→ 摘要 ──→ [Lesson] ──→ 晋升      │  │
│  │                                     [Decision]          │  │
│  │                                                          │  │
│  │  ┌──────────────────────────────────────────────────┐   │  │
│  │  │  微调数据池 (新增)                                │   │  │
│  │  │  training_data/raw/     ← 原始笔记               │   │  │
│  │  │  training_data/processed/ ← 转化的 instruction    │   │  │
│  │  │  training_data/dataset.jsonl ← 最终训练集         │   │  │
│  │  │  lora/adapters/        ← 生成的 LoRA 权重         │   │  │
│  │  └──────────────────────────────────────────────────┘   │  │
│  └──────────────────────────────────────────────────────────┘  │
│                     Tauri 2.0 桌面应用                           │
└─────────────────────────────────────────────────────────────────┘
```

---

## 4. 三层记忆模型

### 4.1 层级定义

```
┌─────────────────────────────────────────────────────────────┐
│  Layer 1: 短期记忆 (Session Context)                         │
│  ─────────────────────────────────────────────────────────  │
│  存储: ~/.neecoder/memory/sessions/<uuid>/messages/*.md     │
│  内容: 原始用户/助手/工具对话消息                              │
│  生命周期: 会话期间活跃，结束后归档                             │
│  Token 预算: 48,000 tokens (滑动窗口)                        │
│  特征: 噪音高，信息密度低，需要 LLM 提炼                       │
│  微调价值: ★☆☆ (不直接使用，需经 Dreaming 过滤)              │
└───────────────────────────┬─────────────────────────────────┘
                            │ Dreaming (本地小模型摘要)
                            ▼
┌─────────────────────────────────────────────────────────────┐
│  Layer 2: 中期记忆 (Daily Notes)                             │
│  ─────────────────────────────────────────────────────────  │
│  存储: ~/.neecoder/memory/notes/YYYY-MM-DD.md               │
│  内容: 每次会话的结构化摘要                                    │
│  格式: - [HH:MM:SS] - [Goal]: xxx                           │
│         - [HH:MM:SS] - [Decision]: xxx                      │
│         - [HH:MM:SS] - [Lesson]: xxx                         │
│  生命周期: 注入 system prompt (今天 + 昨天 + 前天)             │
│  特征: 中等质量，带标签可过滤                                  │
│  微调价值: ★★☆ (instruction tuning 数据源)                   │
└───────────────────────────┬─────────────────────────────────┘
                            │ 质量过滤 (本地小模型评分)
                            ▼
┌─────────────────────────────────────────────────────────────┐
│  Layer 3: 长期记忆 (Long-term Memory)                        │
│  ─────────────────────────────────────────────────────────  │
│  存储: ~/.neecoder/memory/MEMORY.md                         │
│  内容: 精炼的可复用知识 (Lesson + Decision)                  │
│  格式: ## Learned Patterns                                   │
│         - [Lesson] HashMap > Vec for parallel results        │
│         - [Decision] Use tokio::JoinSet for parallel tools  │
│  生命周期: 永久保留，定期整理去重                              │
│  Token 预算: ≤ 2,000 tokens (防止 system prompt 膨胀)        │
│  特征: 高质量，高密度，可直接用于训练                           │
│  微调价值: ★★★ (LoRA 训练核心数据)                           │
└─────────────────────────────────────────────────────────────┘
```

### 4.2 记忆晋升流程

```
                    Dreaming 触发
                         │
                         ▼
            ┌───── 读取 Session 前 20 条消息 ─────┐
            │                                      │
            ▼                                      ▼
   本地小模型生成摘要                      格式化标签提取
   (Qwen2.5-3B Q4)                       [Goal] [Decision] [Lesson]
            │                                      │
            ▼                                      ▼
   追加到 Daily Notes                     包含 [Lesson]/[Decision]？
   notes/2026-06-26.md                           │
            │                          Yes ───────┼────── No
            │                          │          │
            │                          ▼          ▼
            │                  追加到 MEMORY.md   仅存 Daily Notes
            │                  (Layer 3 晋升)
            │
            ▼
   Daily Notes 累计 ≥ 50 条？
         │
    Yes──┼── No
         │  │
         ▼  └── 等待下次会话
   触发微调数据管道
```

### 4.3 记忆过期与清理

```rust
// 新增：记忆生命周期管理
pub struct MemoryGC {
    /// MEMORY.md 最大 token 数（超出触发整理）
    max_memory_tokens: usize,     // 默认 2000
    /// Daily Notes 保留天数
    notes_retention_days: u32,     // 默认 30
    /// 已归档会话保留天数
    archived_session_days: u32,    // 默认 90
}
```

**清理策略**：
1. **MEMORY.md 膨胀**：超过 token 预算时，用本地小模型做"二次提炼"——合并相似条目、删除过时信息
2. **Daily Notes 过期**：超过 30 天的笔记，提取 `[Lesson]` 后删除原文件
3. **Session 归档**：超过 90 天的会话目录，压缩为 `.tar.gz` 存档

---

## 5. 本地大模型集成

### 5.1 模型分层

| 层级 | 用途 | 推荐模型 | 量化 | RAM 需求 | 延迟 |
|------|------|---------|------|---------|------|
| **Dreaming 模型** | 会话摘要、记忆提炼 | Qwen2.5-3B-Instruct | Q4_K_M | 2.5 GB | ~2s/条 |
| **推理模型** | 简单问答、代码补全 | Qwen2.5-7B-Instruct | Q4_K_M | 6 GB | ~500ms |
| **微调基座** | LoRA 训练基座 | Qwen2.5-7B (base) | F16 | 14 GB VRAM | 离线 |
| **Embedding 模型** | 记忆语义搜索 | nomic-embed-text | F16 | 1 GB | ~50ms |

### 5.2 运行时架构

```
┌──────────────────────────────────────────────┐
│            Ollama HTTP Server                │
│            localhost:11434                   │
│                                              │
│  ┌──────────┐  ┌──────────┐  ┌───────────┐  │
│  │ qwen2.5  │  │ qwen2.5  │  │ nomic-    │  │
│  │ :3b      │  │ :7b      │  │ embed-text│  │
│  │ (dream)  │  │ (infer)  │  │ (search)  │  │
│  └──────────┘  └──────────┘  └───────────┘  │
│                                              │
│  ┌──────────────────────────────────────┐    │
│  │  LoRA Adapter (用户个性化)            │    │
│  │  lora/adapters/latest.safetensors    │    │
│  │  挂载到 qwen2.5:7b                    │    │
│  └──────────────────────────────────────┘    │
└──────────────────────────────────────────────┘
         ↑
         │ HTTP (OpenAI 兼容协议)
         │
┌────────┴─────────────────────────────────────┐
│         LLM Router (Rust 后端)                │
│                                              │
│  fn route_llm(task: TaskType) -> LlmConfig  │
│  ┌──────────────────────────────────────────┐│
│  │ Agent 主循环 → 远程 DeepSeek (高质量)    ││
│  │ Dreaming    → 本地 qwen2.5:3b (免费)    ││
│  │ 简单问答     → 本地 qwen2.5:7b+LoRA     ││
│  │ 代码补全     → 远程 DeepSeek (低延迟)    ││
│  │ 记忆搜索     → 本地 nomic-embed-text    ││
│  └──────────────────────────────────────────┘│
│                                              │
│  自动降级: 本地不可用 → fallback 到远程 API   │
└──────────────────────────────────────────────┘
```

### 5.3 LLM Router 设计

```rust
/// 任务类型 → LLM 配置路由
pub enum TaskType {
    AgentMainLoop,      // Agent 主循环推理
    Dreaming,            // 会话摘要
    SimpleChat,          // 简单问答
    CodeCompletion,      // FIM 代码补全
    MemorySearch,        // 语义 embedding
    FinetuneDataGen,     // 微调数据转化
}

pub struct LlmRoute {
    pub provider: LlmProvider,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

impl LlmRouter {
    pub fn route(&self, task: TaskType, local_config: &LocalModelConfig) -> LlmRoute {
        match task {
            TaskType::AgentMainLoop => {
                // 始终用远程（需要高质量推理 + 工具调用）
                self.remote_config()
            }
            TaskType::Dreaming => {
                // 优先本地（省钱 + 隐私）
                if local_config.available {
                    LlmRoute {
                        provider: LlmProvider::Ollama,
                        base_url: local_config.base_url.clone(),
                        api_key: String::new(),
                        model: local_config.dreaming_model.clone(),
                        temperature: 0.3,
                        max_tokens: 300,
                    }
                } else {
                    self.remote_config()
                }
            }
            TaskType::SimpleChat => {
                // 本地 + LoRA 个性化
                if local_config.available && local_config.lora_loaded {
                    self.local_with_lora(local_config)
                } else {
                    self.remote_config()
                }
            }
            TaskType::CodeCompletion => {
                // 远程优先（低延迟需求）
                self.remote_config()
            }
            TaskType::MemorySearch => {
                // 本地 embedding
                self.local_embedding(local_config)
            }
            TaskType::FinetuneDataGen => {
                // 本地小模型生成训练数据
                self.local_dreaming(local_config)
            }
        }
    }
}
```

### 5.4 健康检查

```rust
pub struct LocalModelHealth {
    pub ollama_running: bool,
    pub models_loaded: Vec<String>,
    pub lora_adapter: Option<String>,
    pub gpu_available: bool,
    pub vram_total_mb: u64,
    pub vram_used_mb: u64,
}

impl LocalModelHealth {
    /// 启动时检查 Ollama 是否可用
    pub async fn check() -> Self {
        // GET http://localhost:11434/api/tags
        // 返回已加载模型列表
    }

    /// 每 30 秒轮询健康状态
    pub fn start_monitoring(&self, app: &AppHandle) {
        // 周期性检查，状态变化时 emit "local-model-status" 事件
    }
}
```

---

## 6. Dreaming 管道

### 6.1 改造后的 Dreaming 流程

```
Agent 完成
    │
    ▼
┌─────────────────────────────────────────┐
│  Step 1: 收集会话消息                    │
│  读取 session 前 20 条消息               │
│  过滤: 只保留 User + Assistant 消息      │
│  截断: 每条消息 ≤ 500 字符                │
└──────────────────┬──────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────┐
│  Step 2: LLM 摘要 (优先本地模型)         │
│                                         │
│  Prompt:                                │
│  "Summarize this coding session in      │
│   3-5 concise bullet points..."         │
│                                         │
│  路由: TaskType::Dreaming                │
│  模型: qwen2.5:3b (本地) / deepseek-chat │
│  参数: temperature=0.3, max_tokens=300  │
└──────────────────┬──────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────┐
│  Step 3: 标签解析与分类                  │
│                                         │
│  提取标签: [Goal] [Decision] [Lesson]    │
│                                         │
│  分类:                                  │
│  ├── 所有摘要 → Daily Notes (Layer 2)   │
│  └── [Lesson]+[Decision] → MEMORY.md    │
│      (Layer 3 晋升)                     │
└──────────────────┬──────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────┐
│  Step 4: 质量评估 (新增)                 │
│                                         │
│  对每条 [Lesson]/[Decision] 打分:        │
│  - 通用性: 是否可跨项目复用？              │
│  - 具体性: 是否过于模糊？                  │
│  - 新颖性: 是否与已有记忆重复？            │
│                                         │
│  低分条目标记为 "draft"，不晋升            │
│  高分条目标记为 "verified"，晋升           │
└──────────────────┬──────────────────────┘
                   │
                   ▼
┌─────────────────────────────────────────┐
│  Step 5: 写入存储                        │
│                                         │
│  Daily Notes: notes/2026-06-26.md       │
│  MEMORY.md: 追加到 "## Learned Patterns" │
│                                         │
│  同时写入微调数据池:                      │
│  training_data/raw/YYYY-MM-DD.md        │
└─────────────────────────────────────────┘
```

### 6.2 质量评估 Prompt

```text
You are a memory quality evaluator. Rate each entry on a scale of 1-5.

## Scoring Criteria
- Generality (1-5): Can this be reused across different projects?
- Specificity (1-5): Is the advice concrete enough to be actionable?
- Novelty (1-5): Is this a common knowledge or a unique insight?

## Entries to Evaluate
{entries}

## Output Format (JSON)
[
  {"index": 0, "generality": 4, "specificity": 3, "novelty": 5, "verdict": "verified"},
  {"index": 1, "generality": 2, "specificity": 1, "novelty": 1, "verdict": "draft"}
]
```

### 6.3 深度 Dreaming（Deep Dreaming）

每周或累计 50 条笔记时触发一次**深度整合**：

```
Step 1: 读取所有 Daily Notes (notes/*.md)
Step 2: 读取 MEMORY.md 全文
Step 3: 本地小模型做全局分析:
  - 识别重复/相似条目 → 合并
  - 识别过时信息 → 标记删除
  - 识别碎片知识 → 归类到对应章节
Step 4: 重写 MEMORY.md (精简 + 结构化)
Step 5: 生成 "Deep Dreaming Report" → 前端展示
```

---

## 7. 微调数据管道

### 7.1 数据来源与质量分级

```
                    数据来源
                        │
        ┌───────────────┼───────────────┐
        ▼               ▼               ▼
  MEMORY.md        Daily Notes     Session Messages
  (Layer 3)        (Layer 2)       (Layer 1)
        │               │               │
        ▼               ▼               ▼
   质量分级 ★★★      质量分级 ★★☆     质量分级 ★☆☆
        │               │               │
        ▼               ▼               ▼
  直接作为            经质量评估       经 Dreaming
  训练核心数据         筛选后使用        摘要后使用
```

### 7.2 训练数据转化

#### 7.2.1 从 MEMORY.md 转化

```markdown
## MEMORY.md 原文

## Learned Patterns

- [Lesson] 使用 tokio::JoinSet 实现并行工具执行时，必须用 HashMap<usize, T> 而非 Vec 存结果，因为 JoinSet::join_next() 返回顺序不确定
```

**转化为 JSONL 训练数据**：

```json
{
  "instruction": "在 Rust 异步编程中，使用 tokio::JoinSet 实现并行任务时，结果应该用什么容器存储？为什么不能用 Vec？",
  "input": "",
  "output": "应使用 HashMap<usize, T> 而非 Vec。因为 JoinSet::join_next() 按\"谁先完成谁先返回\"的顺序返回结果，不是按 spawn 顺序。用 Vec 存储会导致索引错位，而 HashMap 通过原始索引 key 可以正确匹配结果。\n\n示例：\n```rust\nlet mut results = HashMap::new();\nresults.insert(0, result_0);\nresults.insert(1, result_1);\n// 串行循环时: results.get(&i) 正确匹配\n```"
}
```

#### 7.2.2 转化流程

```
Step 1: 解析 MEMORY.md
        提取所有 [Lesson] 和 [Decision] 条目
        每条记录原始文本 + 标签类型

Step 2: 生成 instruction (本地小模型)
        输入: "- [Lesson] HashMap > Vec for parallel results"
        输出: {"instruction": "在 Rust 并行编程中，JoinSet 的结果应该用什么容器存储？"}

Step 3: 生成 output (本地小模型)
        输入: 原始 lesson 文本 + instruction
        输出: 扩展的回答（补充上下文、示例代码）

Step 4: 质量过滤
        - instruction 长度 ≥ 10 字符
        - output 长度 ≥ 50 字符
        - 无重复（embedding 相似度 < 0.9）
        - 无敏感信息（API key、密码等）

Step 5: 写入 dataset.jsonl
```

### 7.3 训练数据格式

```jsonl
{"instruction": "...", "input": "", "output": "...", "category": "rust/async"}
{"instruction": "...", "input": "...", "output": "...", "category": "architecture"}
{"instruction": "...", "input": "", "output": "...", "category": "debugging"}
```

**字段说明**：
- `instruction`: 问题/指令
- `input`: 可选的输入上下文（如代码片段）
- `output`: 期望的回答
- `category`: 知识分类（用于按类采样）

### 7.4 数据增强

对高质量条目做增强，增加训练数据多样性：

| 增强方式 | 方法 | 示例 |
|---------|------|------|
| **改写** | 本地小模型改写 instruction | "为什么用 HashMap？" → "Vec 和 HashMap 哪个适合存并行结果？" |
| **补充示例** | 让模型生成更多代码示例 | 为 lesson 生成 2-3 个不同的代码示例 |
| **反向提问** | 生成反向问题 | "什么时候不该用 HashMap？" |
| **上下文注入** | 注入项目特定上下文 | "在 NeeCoder 项目中..." |

---

## 8. LoRA 微调系统

### 8.1 微调架构

```
┌──────────────────────────────────────────────────┐
│              微调触发器 (Trigger)                  │
│                                                  │
│  策略 1: 阈值触发 — notes ≥ 50 条                │
│  策略 2: 定时触发 — 每周日凌晨 3:00               │
│  策略 3: 手动触发 — 用户点击 "Train" 按钮         │
│                                                  │
│  前置检查:                                        │
│  ├── Ollama 运行中？                              │
│  ├── GPU 可用？ (fallback: CPU)                  │
│  ├── 基座模型已下载？                             │
│  └── 训练数据 ≥ 20 条？                           │
└──────────────────────┬───────────────────────────┘
                       │
                       ▼
┌──────────────────────────────────────────────────┐
│              数据准备 (Data Prep)                 │
│                                                  │
│  1. 读取 MEMORY.md + notes/*.md                  │
│  2. 本地小模型转化 → instruction-response 对       │
│  3. 质量过滤 + 去重                               │
│  4. 数据增强                                       │
│  5. 输出: dataset.jsonl                           │
│                                                  │
│  进度事件: "finetune-progress" (stage: data_prep)  │
└──────────────────────┬───────────────────────────┘
                       │
                       ▼
┌──────────────────────────────────────────────────┐
│              LoRA 训练 (Training)                 │
│                                                  │
│  基座模型: qwen2.5:7b (GGUF F16)                 │
│  方法: LoRA (rank=8, alpha=16, dropout=0.05)     │
│  Epochs: 3                                       │
│  学习率: 2e-4 (cosine schedule)                   │
│  Batch size: 4 (梯度累积 4 = 有效 16)             │
│  最大序列长度: 2048                                │
│                                                  │
│  实现方式:                                         │
│  ├── 方案 A: 内置 llama.cpp 训练 (推荐)           │
│  ├── 方案 B: 调用外部 Python 脚本 (unsloth/peft)  │
│  └── 方案 C: 调用 Ollama 微调 API (如可用)        │
│                                                  │
│  进度事件: "finetune-progress" (stage: training)   │
│  输出: lora/adapters/{date}/adapter.safetensors   │
└──────────────────────┬───────────────────────────┘
                       │
                       ▼
┌──────────────────────────────────────────────────┐
│              模型评估 (Evaluation)                │
│                                                  │
│  1. 构建测试集 (20% 数据留出)                     │
│  2. 微调前后对比:                                 │
│     - 准确率 (关键词匹配)                          │
│     - 流畅度 (perplexity)                         │
│     - 相关性 (embedding 相似度)                    │
│  3. 评估报告 → 前端展示                            │
│                                                  │
│  进度事件: "finetune-progress" (stage: eval)       │
└──────────────────────┬───────────────────────────┘
                       │
                       ▼
┌──────────────────────────────────────────────────┐
│              部署 (Deploy)                       │
│                                                  │
│  1. 如果评估通过 → 激活新 adapter                  │
│  2. Ollama 加载: qwen2.5:7b + adapter             │
│  3. 更新配置: lora_adapter_path → 新路径          │
│  4. 旧 adapter 保留 (支持回滚)                     │
│  5. emit "finetune-complete" 事件                  │
│                                                  │
│  前端通知: "个性化模型已更新"                       │
└──────────────────────────────────────────────────┘
```

### 8.2 LoRA 参数选择

| 参数 | 推荐值 | 说明 |
|------|--------|------|
| rank (r) | 8 | 平衡参数量和表达能力 |
| alpha | 16 | 通常为 rank 的 2 倍 |
| dropout | 0.05 | 防止过拟合 |
| target_modules | q_proj, v_proj | 只训练注意力层 |
| epochs | 3 | 小数据集多轮 |
| learning_rate | 2e-4 | cosine schedule |
| batch_size | 4 | 梯度累积 4 → 有效 16 |
| max_seq_length | 2048 | 覆盖大多数代码场景 |

### 8.3 Adapter 版本管理

```
~/.neecoder/lora/
├── adapters/
│   ├── 2026-06-20/
│   │   ├── adapter.safetensors
│   │   ├── config.json          # LoRA 配置
│   │   ├── eval_report.json     # 评估报告
│   │   └── training_data.jsonl  # 训练数据快照
│   ├── 2026-06-27/
│   │   ├── ...
│   └── latest -> 2026-06-27/    # 软链接指向最新
├── dataset.jsonl                # 当前训练集
└── training_history.json         # 训练历史记录
```

### 8.4 训练历史追踪

```json
// training_history.json
{
  "sessions": [
    {
      "id": "2026-06-20-001",
      "timestamp": "2026-06-20T03:00:00Z",
      "trigger": "weekly",
      "data_samples": 45,
      "epochs": 3,
      "train_loss": [1.2, 0.8, 0.6],
      "eval_accuracy": 0.82,
      "eval_perplexity": 15.3,
      "adapter_path": "adapters/2026-06-20/",
      "status": "deployed"
    },
    {
      "id": "2026-06-27-002",
      "timestamp": "2026-06-27T03:00:00Z",
      "trigger": "threshold",
      "data_samples": 62,
      "epochs": 3,
      "train_loss": [1.1, 0.7, 0.5],
      "eval_accuracy": 0.86,
      "eval_perplexity": 12.1,
      "adapter_path": "adapters/2026-06-27/",
      "status": "deployed"
    }
  ],
  "active_adapter": "2026-06-27-002"
}
```

---

## 9. 配置设计

### 9.1 新增配置结构

```rust
// config/mod.rs 新增

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModelConfig {
    /// 是否启用本地模型
    pub enabled: bool,
    /// Ollama 服务地址
    pub base_url: String,
    /// Dreaming 专用模型 (小模型，低延迟)
    pub dreaming_model: String,
    /// 推理模型 (中等模型，日常问答)
    pub inference_model: String,
    /// 微调基座模型
    pub finetune_base_model: String,
    /// Embedding 模型 (记忆语义搜索)
    pub embedding_model: String,
    /// LoRA adapter 路径
    pub lora_adapter_path: Option<String>,
    /// 是否使用 LoRA 个性化
    pub lora_enabled: bool,
}

impl Default for LocalModelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "http://localhost:11434".into(),
            dreaming_model: "qwen2.5:3b".into(),
            inference_model: "qwen2.5:7b".into(),
            finetune_base_model: "qwen2.5:7b".into(),
            embedding_model: "nomic-embed-text".into(),
            lora_adapter_path: None,
            lora_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FineTuneConfig {
    /// 是否启用微调
    pub enabled: bool,
    /// 触发策略
    pub trigger: FineTuneTrigger,
    /// 阈值触发：累计笔记条数
    pub threshold_count: u32,
    /// 定时触发：Cron 表达式
    pub schedule_cron: String,
    /// LoRA rank
    pub lora_rank: u32,
    /// LoRA alpha
    pub lora_alpha: u32,
    /// 训练轮数
    pub epochs: u32,
    /// 学习率
    pub learning_rate: f64,
    /// 最大序列长度
    pub max_seq_length: u32,
    /// 是否使用 GPU
    pub use_gpu: bool,
    /// 输出目录
    pub output_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FineTuneTrigger {
    /// 手动触发
    Manual,
    /// 阈值触发
    Threshold,
    /// 定时触发
    Scheduled,
}

impl Default for FineTuneConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            trigger: FineTuneTrigger::Manual,
            threshold_count: 50,
            schedule_cron: "0 3 * * 0".into(), // 每周日凌晨 3 点
            lora_rank: 8,
            lora_alpha: 16,
            epochs: 3,
            learning_rate: 2e-4,
            max_seq_length: 2048,
            use_gpu: true,
            output_dir: "~/.neecoder/lora".into(),
        }
    }
}
```

### 9.2 AppSettings 扩展

```rust
pub struct AppSettings {
    // ... 现有字段 ...
    
    /// 本地模型配置 (新增)
    #[serde(default)]
    pub local_model: LocalModelConfig,
    
    /// 微调配置 (新增)
    #[serde(default)]
    pub fine_tune: FineTuneConfig,
    
    /// 记忆管理配置 (新增)
    #[serde(default)]
    pub memory_gc: MemoryGCConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryGCConfig {
    /// MEMORY.md 最大 token 数
    pub max_memory_tokens: usize,
    /// Daily Notes 保留天数
    pub notes_retention_days: u32,
    /// 会话保留天数
    pub session_retention_days: u32,
    /// 是否启用语义搜索 (需要 embedding 模型)
    pub semantic_search: bool,
}

impl Default for MemoryGCConfig {
    fn default() -> Self {
        Self {
            max_memory_tokens: 2000,
            notes_retention_days: 30,
            session_retention_days: 90,
            semantic_search: false,
        }
    }
}
```

---

## 10. 数据流与状态机

### 10.1 完整闭环数据流

```
                    ┌─────────────┐
                    │  用户交互    │
                    └──────┬──────┘
                           │
                    ┌──────▼──────┐
                    │ Agent 主循环  │ ← 远程 LLM
                    │ (高质量推理)  │
                    └──────┬──────┘
                           │
                    ┌──────▼──────┐
                    │  会话消息    │ → Layer 1 (短期)
                    │  (Session)   │
                    └──────┬──────┘
                           │ Agent 完成
                    ┌──────▼──────┐
                    │  Dreaming   │ ← 本地小模型
                    │  (摘要+评估)  │
                    └──┬─────┬────┘
                       │     │
               ┌───────▼─┐ ┌─▼──────────┐
               │ Daily   │ │ MEMORY.md  │
               │ Notes   │ │ (Layer 3)  │
               │(Layer 2)│ └──────┬─────┘
               └────┬────┘        │
                    │             │
                    │     ┌───────▼───────┐
                    │     │ 微调数据池      │
                    │     │ dataset.jsonl  │
                    │     └───────┬───────┘
                    │             │
                    │     ┌───────▼───────┐
                    │     │ LoRA 微调     │ ← 本地基座模型
                    │     │ (离线训练)     │
                    │     └───────┬───────┘
                    │             │
                    │     ┌───────▼───────┐
                    │     │ 个性化 Adapter  │
                    │     │ (.safetensors) │
                    │     └───────┬───────┘
                    │             │
                    │     ┌───────▼───────┐
                    └────→│ 本地推理模型    │
                          │ (base + LoRA)  │
                          └───────┬───────┘
                                  │
                                  │ 简单问答/补全
                          ┌───────▼───────┐
                          │  用户交互      │ ← 个性化回答
                          └───────────────┘
                                  │
                                  └──→ 新的会话 → 正向循环 ↺
```

### 10.2 微调状态机

```
                    ┌──────────┐
                    │  Idle    │ ← 默认状态
                    └─────┬────┘
                          │ trigger_finetune()
                          ▼
                    ┌──────────┐
                    │ Checking │ ← 检查前置条件
                    │          │   (Ollama/GPU/数据量)
                    └─────┬────┘
                          │ 条件满足
                          ▼
                    ┌──────────┐
                    │Data Prep │ ← 构建训练数据
                    │          │   (转化+过滤+增强)
                    └─────┬────┘
                          │ 数据就绪
                          ▼
                    ┌──────────┐     失败/中断
                    │Training  │──────────────┐
                    │          │              │
                    └─────┬────┘              │
                          │ 训练完成            │
                          ▼                    │
                    ┌──────────┐              │
                    │Evaluate  │              │
                    │          │              │
                    └─────┬────┘              │
                          │                   │
                    ┌─────▼─────┐             │
                    │ 评估通过？ │             │
                    └──┬────┬───┘             │
                  Yes  │    │ No              │
                       ▼    ▼                 │
              ┌────────┐ ┌────────┐           │
              │Deploy  │ │Reject │           │
              │(激活)   │ │(保留旧) │           │
              └───┬────┘ └───┬────┘           │
                  │          │                │
                  ▼          ▼                ▼
              ┌──────────────────────────────────┐
              │            Idle                   │
              └──────────────────────────────────┘
```

---

## 11. 前端交互设计

### 11.1 新增前端组件

```
Settings 页面新增:
├── "本地模型" Tab (新增)
│   ├── Ollama 状态指示器 (绿色/红色)
│   ├── 已加载模型列表
│   ├── GPU/CPU 切换
│   ├── LoRA adapter 状态 (当前版本/评估分数)
│   └── 测试连接按钮
│
├── "记忆管理" Tab (新增)
│   ├── MEMORY.md 预览 (只读)
│   ├── Daily Notes 浏览器 (日期选择)
│   ├── 记忆统计 (条目数/分类/趋势图)
│   ├── 手动清理按钮
│   └── 导出训练数据按钮
│
└── "模型微调" Tab (新增)
    ├── 训练数据统计 (条目数/质量分布)
    ├── 微调参数配置 (rank/alpha/epochs)
    ├── 触发方式 (手动/阈值/定时)
    ├── 训练进度条 (Data Prep → Training → Eval → Deploy)
    ├── 训练历史 (版本列表 + 评估报告)
    └── 回滚按钮 (切换到旧 adapter)

ChatPanel 新增:
├── 状态栏指示器: 🟢 本地模型可用 / 🔴 不可用
├── 消息标注: "🧠 个性化回答 (LoRA)" 标签
└── Dreaming 状态: "💤 正在整理记忆..." 进度提示
```

### 11.2 新增 Tauri 事件

| 事件名 | 触发时机 | Payload |
|--------|---------|---------|
| `local-model-status` | 本地模型状态变化 | `{ available, models, gpu, lora_loaded }` |
| `dreaming-progress` | Dreaming 进行中 | `{ stage, message }` |
| `finetune-progress` | 微调进行中 | `{ stage, progress, message }` |
| `finetune-complete` | 微调完成 | `{ adapter_path, eval_report }` |
| `memory-updated` | 记忆文件更新 | `{ layer, entries_count }` |

### 11.3 新增 Tauri 命令

| 命令 | 功能 |
|------|------|
| `check_local_model` | 检查 Ollama 健康状态 |
| `get_memory_stats` | 获取记忆统计信息 |
| `export_training_data` | 导出训练数据为 JSONL |
| `trigger_finetune` | 手动触发微调 |
| `get_finetune_history` | 获取微调历史 |
| `rollback_adapter` | 回滚到旧 adapter |
| `preview_memory` | 预览 MEMORY.md 内容 |
| `clean_memory` | 清理过期记忆 |
| `semantic_search_memory` | 语义搜索记忆 (使用本地 embedding) |

---

## 12. 实施计划

### Phase 1: 本地模型集成 (2 周)

**目标**: 让 Dreaming 走本地模型

| 任务 | 文件 | 说明 |
|------|------|------|
| 新增 LocalModelConfig | `config/mod.rs` | 配置结构 + 序列化 |
| 新增 LlmRouter | `llm/mod.rs` (新文件 `llm/router.rs`) | 任务类型路由 |
| 改造 dreaming() | `memory/mod.rs` | 支持本地模型参数 |
| 健康检查 | `llm/health.rs` (新文件) | Ollama 状态检测 |
| 前端设置 UI | `Settings.tsx` | 本地模型配置面板 |
| 验证 | 手动测试 | Dreaming 走本地 |

### Phase 2: 记忆增强 (2 周)

**目标**: 语义搜索 + 记忆清理 + 质量评估

| 任务 | 文件 | 说明 |
|------|------|------|
| 语义搜索 | `memory/search.rs` | 用 embedding 替代关键词 |
| 记忆质量评估 | `memory/mod.rs` | Dreaming 中增加打分 |
| 记忆 GC | `memory/gc.rs` (新文件) | 过期清理 + 膨胀控制 |
| Deep Dreaming | `memory/mod.rs` | 周期性全局整合 |
| 前端记忆面板 | `MemoryPanel.tsx` (新组件) | 浏览 + 统计 + 管理 |
| 验证 | 单元测试 + 手动测试 | 搜索准确率 + GC 效果 |

### Phase 3: 微调数据管道 (2 周)

**目标**: 从记忆自动构建训练数据集

| 任务 | 文件 | 说明 |
|------|------|------|
| 数据转化器 | `memory/finetune/data.rs` (新文件) | MEMORY.md → JSONL |
| 数据增强 | `memory/finetune/augment.rs` (新文件) | 改写 + 反向提问 |
| 质量过滤 | `memory/finetune/filter.rs` (新文件) | 去重 + 评分 |
| 导出命令 | `commands/finetune.rs` (新文件) | Tauri 命令 |
| 前端微调面板 | `FinetunePanel.tsx` (新组件) | 数据统计 + 参数 |
| 验证 | 数据集质量评估 | 覆盖率 + 去重率 |

### Phase 4: LoRA 微调 (3 周)

**目标**: 端到端微调 + 部署 + 回滚

| 任务 | 文件 | 说明 |
|------|------|------|
| 训练调度器 | `memory/finetune/trainer.rs` (新文件) | 触发 + 前置检查 |
| 训练执行 | 外部脚本 / llama.cpp | LoRA 训练 |
| 模型评估 | `memory/finetune/eval.rs` (新文件) | 准确率 + perplexity |
| Adapter 部署 | `llm/adapter.rs` (新文件) | Ollama 加载 + 切换 |
| 版本管理 | `memory/finetune/history.rs` | 历史 + 回滚 |
| 前端训练面板 | `FinetunePanel.tsx` | 进度 + 历史 + 回滚 |
| 验证 | 端到端测试 | 微调 → 评估 → 部署 → 推理 |

### Phase 5: 优化与打磨 (2 周)

**目标**: 体验优化 + 降级容错

| 任务 | 说明 |
|------|------|
| 优雅降级 | 本地不可用时自动 fallback |
| 进度通知 | 所有异步任务的前端通知 |
| 日志完善 | 微调全过程日志 |
| 文档 | 用户使用指南 |
| 性能优化 | 大文件处理 + 并发安全 |

---

## 13. 技术挑战与对策

### 13.1 挑战矩阵

| 挑战 | 难度 | 对策 |
|------|------|------|
| Ollama 不可用时降级 | ★★☆ | LlmRouter 自动 fallback 到远程 API |
| 无 GPU 用户微调 | ★★★ | CPU fallback (慢但可用) 或云端微调选项 |
| 训练数据质量差 | ★★★ | 三级过滤：Dreaming 质量评估 + 数据增强去重 + 人工审核 |
| MEMORY.md 并发写 | ★★☆ | 从 Read-Modify-Write 改为 Append-Only + 定期 Compact |
| LoRA 兼容性 | ★★☆ | 版本绑定 (adapter + base_model 版本匹配) |
| 模型下载/管理 | ★★☆ | 内置 Ollama 安装向导 + 模型下载进度 |
| 隐私安全 | ★☆☆ | 全本地处理，不上传任何记忆数据 |

### 13.2 关键技术决策

**决策 1: 微调执行方式**

| 方案 | 优点 | 缺点 | 推荐 |
|------|------|------|------|
| A. 内置 llama.cpp | 无外部依赖 | 复杂度高 | Phase 4 初期 |
| B. 外部 Python 脚本 | 成熟生态 | 需 Python 环境 | ★ 推荐 |
| C. Ollama API | 最简单 | 功能不确定 | 观察 |

**推荐方案 B**：打包一个 Python 虚拟环境，用 `unsloth` 或 `peft` 做微调。Tauri 通过 `tokio::process::Command` 调用外部脚本。

**决策 2: Embedding 存储**

| 方案 | 优点 | 缺点 |
|------|------|------|
| 内存 HashMap | 快 | 重启丢失 |
| SQLite + 向量 | 持久化 | 复杂 |
| 文件 + 内存索引 | 简单 | 启动加载慢 |

**推荐**：复用现有 SQLite（RAG 已用），新增 memory_embeddings 表。

---

## 14. 评估体系

### 14.1 记忆质量指标

| 指标 | 测量方法 | 目标 |
|------|---------|------|
| 搜索准确率 | 人工标注 20 条查询 | Top-5 命中率 ≥ 80% |
| MEMORY.md token 利用率 | 有用条目 / 总 token | ≥ 70% |
| Daily Notes 冗余率 | 重复条目比例 | ≤ 10% |
| Dreaming 摘要质量 | 人工评分 (1-5) | 平均 ≥ 3.5 |

### 14.2 微调效果指标

| 指标 | 测量方法 | 目标 |
|------|---------|------|
| 训练损失收敛 | loss 曲线 | 最终 < 0.8 |
| 测试集准确率 | 关键词匹配 | ≥ 75% |
| Perplexity 降低 | 微调前后对比 | 降低 ≥ 15% |
| 个性化增益 | 用户代码问题回答质量 | 人工评分提升 ≥ 1 分 |

### 14.3 系统健康指标

| 指标 | 测量方法 | 目标 |
|------|---------|------|
| 本地模型可用率 | 30s 轮询 | ≥ 95% |
| Dreaming 成功率 | 成功/总次数 | ≥ 90% |
| 微调完成率 | 成功/触发次数 | ≥ 80% |
| 端到端延迟 | Agent 完成 → Dreaming 完成 | ≤ 10s |
| 内存占用峰值 | 进程 RSS | ≤ 8 GB (推理) |

---

## 附录 A: 文件系统结构

```
~/.neecoder/
├── memory/
│   ├── MEMORY.md                    # Layer 3: 长期记忆
│   ├── notes/                       # Layer 2: 每日笔记
│   │   ├── 2026-06-25.md
│   │   ├── 2026-06-26.md
│   │   └── ...
│   ├── sessions/                    # Layer 1: 会话消息
│   │   ├── <uuid>/
│   │   │   ├── session.md
│   │   │   └── messages/
│   │   │       ├── 00000001.md
│   │   │       └── 00000002.md
│   │   └── ...
│   └── embeddings.db               # 记忆向量索引 (SQLite, 新增)
│
├── lora/                            # 微调相关 (新增)
│   ├── dataset.jsonl                # 当前训练数据集
│   ├── training_history.json        # 训练历史
│   ├── adapters/                    # LoRA 权重版本
│   │   ├── 2026-06-20/
│   │   │   ├── adapter.safetensors
│   │   │   ├── config.json
│   │   │   ├── eval_report.json
│   │   │   └── training_data.jsonl
│   │   ├── 2026-06-27/
│   │   │   └── ...
│   │   └── latest -> 2026-06-27/    # 软链接
│   └── scripts/                     # 微调脚本
│       ├── prepare_data.py          # 数据准备
│       ├── train_lora.py            # LoRA 训练
│       └── evaluate.py              # 模型评估
│
├── config/
│   └── settings.json                # 配置 (含 local_model + fine_tune)
│
└── logs/
    └── neecoder.log                 # 日志
```

## 附录 B: 新增 Rust 模块结构

```
src-tauri/src/
├── memory/
│   ├── mod.rs                       # MemoryManager (改造)
│   ├── session_store.rs             # (现有)
│   ├── long_term.rs                 # (现有)
│   ├── notes.rs                     # (现有)
│   ├── search.rs                    # (改造: 支持语义搜索)
│   ├── tools.rs                     # (现有)
│   ├── gc.rs                        # (新增) 记忆垃圾回收
│   └── finetune/                    # (新增) 微调子系统
│       ├── mod.rs                   # 模块入口
│       ├── data.rs                  # 训练数据转化
│       ├── augment.rs               # 数据增强
│       ├── filter.rs                # 质量过滤
│       ├── trainer.rs               # 训练调度
│       ├── eval.rs                  # 模型评估
│       └── history.rs              # 版本管理
│
├── llm/
│   ├── mod.rs                       # (现有)
│   ├── router.rs                    # (新增) LLM 路由
│   ├── health.rs                    # (新增) 健康检查
│   └── adapter.rs                   # (新增) LoRA adapter 管理
│
├── config/
│   └── mod.rs                       # (改造) 新增配置结构
│
└── commands/
    ├── chat.rs                      # (改造) dreaming 支持本地模型
    └── finetune.rs                  # (新增) 微调相关命令
```

## 附录 C: Python 微调脚本接口

### `scripts/prepare_data.py`
```python
# 输入: memory/MEMORY.md, memory/notes/*.md
# 输出: lora/dataset.jsonl
# 参数: --memory-dir, --output, --min-quality, --augment
```

### `scripts/train_lora.py`
```python
# 输入: lora/dataset.jsonl
# 输出: lora/adapters/{date}/adapter.safetensors
# 参数: --base-model, --rank, --alpha, --epochs, --lr, --output-dir
# 依赖: unsloth 或 peft + transformers
```

### `scripts/evaluate.py`
```python
# 输入: adapter + 测试集
# 输出: lora/adapters/{date}/eval_report.json
# 参数: --adapter-path, --test-data, --base-model
# 指标: accuracy, perplexity, relevance_score
```
