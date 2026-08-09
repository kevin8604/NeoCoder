import { useState, useEffect, useRef } from "react";
import { Search, Plug, Bot, Zap } from "lucide-react";
import {
  getAppLogs,
  getLogPath,
  listMcpServers,
  connectMcpServer,
  disconnectMcpServer,
  getAgents,
  saveAgent,
  deleteAgent,
  listAvailableTools,
  listSkills,
  saveSkill,
  deleteSkill,
  checkLocalModel,
  cleanupMemory,
  runDeepDreaming,
  exportTrainingData,
  type LocalModelHealth,
  type McpServerStatus,
  type AgentDefinition,
  type SkillDefinition,
  type ToolInfo,
} from "../hooks/useTauri";

/** Map model name to context window size (mirrors backend config::model_context_window) */
function modelContextWindow(model: string): string {
  const m = model.toLowerCase();
  if (m.includes("deepseek-v4") || m.includes("deepseek-v3")) return "128K";
  if (m.includes("deepseek")) return "64K";
  if (m.includes("claude-3.5-sonnet") || m.includes("claude-3.5-haiku")) return "200K";
  if (m.includes("claude-3-opus") || m.includes("claude-3-sonnet")) return "200K";
  if (m.includes("gpt-4o") || m.includes("gpt-4-turbo")) return "128K";
  if (m.includes("gpt-4")) return "8K";
  if (m.includes("gpt-3.5")) return "16K";
  if (m.includes("qwen")) return "128K";
  if (m.includes("llama-3") || m.includes("llama3")) return "128K";
  return "32K";
}

interface SettingsData {
  llm_provider: string;
  completion_model: string;
  chat_model: string;
  embedding_model: string;
  fast_model: string;
  model_routing_enabled: boolean;
  thinking_enabled: boolean;
  thinking_budget: number;
  api_key: string;
  completion_enabled: boolean;
  trigger_debounce_ms: number;
  max_context_tokens: number;
  custom_instructions: string;
  theme: string;
  auto_review_on_save: boolean;
  auto_review_on_commit: boolean;
  // ── Local model (Ollama) routing ──
  local_model: {
    enabled: boolean;
    base_url: string;
    dreaming_model: string;
    inference_model: string;
    embedding_model: string;
  };
  // ── Memory GC / semantic search ──
  memory_gc: {
    max_memory_tokens: number;
    notes_retention_days: number;
    session_retention_days: number;
    semantic_search: boolean;
  };
  // ── Fine-tune data pipeline ──
  fine_tune: {
    enabled: boolean;
    trigger: string;
    threshold_count: number;
    lora_rank: number;
    lora_alpha: number;
    epochs: number;
    learning_rate: number;
    use_gpu: boolean;
  };
}

export default function Settings() {
  const [settings, setSettings] = useState<SettingsData>({
    llm_provider: "deepseek",
    completion_model: "deepseek-chat",
    chat_model: "deepseek-chat",
    embedding_model: "text-embedding-3-small",
    fast_model: "deepseek-chat",
    model_routing_enabled: false,
    thinking_enabled: false,
    thinking_budget: 1024,
    api_key: "",
    completion_enabled: true,
    trigger_debounce_ms: 300,
    max_context_tokens: 8192,
    custom_instructions: "",
    theme: "Dark",
    auto_review_on_save: false,
    auto_review_on_commit: false,
    local_model: {
      enabled: false,
      base_url: "http://localhost:11434",
      dreaming_model: "qwen2.5:3b",
      inference_model: "qwen2.5:7b",
      embedding_model: "nomic-embed-text",
    },
    memory_gc: {
      max_memory_tokens: 2000,
      notes_retention_days: 30,
      session_retention_days: 90,
      semantic_search: false,
    },
    fine_tune: {
      enabled: false,
      trigger: "manual",
      threshold_count: 50,
      lora_rank: 8,
      lora_alpha: 16,
      epochs: 3,
      learning_rate: 0.0002,
      use_gpu: true,
    },
  });

  const [saved, setSaved] = useState(false);
  const [logContent, setLogContent] = useState("");
  const [logPath, setLogPath] = useState("");
  const [logOpen, setLogOpen] = useState(false);
  const [logLoading, setLogLoading] = useState(false);
  const logRef = useRef<HTMLPreElement>(null);

  // ── MCP state ──
  const [mcpServers, setMcpServers] = useState<McpServerStatus[]>([]);
  const [mcpOpen, setMcpOpen] = useState(false);
  const [mcpLoading, setMcpLoading] = useState(false);
  const [mcpError, setMcpError] = useState<string | null>(null);
  const [addMcpOpen, setAddMcpOpen] = useState(false);
  const [newMcpName, setNewMcpName] = useState("");
  const [newMcpCommand, setNewMcpCommand] = useState("");
  const [newMcpArgs, setNewMcpArgs] = useState("");
  const [newMcpEnv, setNewMcpEnv] = useState("");

  // ── Agent management state ──
  const [agentsOpen, setAgentsOpen] = useState(false);
  const [agents, setAgents] = useState<AgentDefinition[]>([]);
  const [agentsLoading, setAgentsLoading] = useState(false);
  const [agentFormOpen, setAgentFormOpen] = useState(false);
  const [editAgent, setEditAgent] = useState<AgentDefinition | null>(null);
  const [agentFormId, setAgentFormId] = useState("");
  const [agentFormName, setAgentFormName] = useState("");
  const [agentFormDesc, setAgentFormDesc] = useState("");
  const [agentFormPrompt, setAgentFormPrompt] = useState("");
  const [agentFormTools, setAgentFormTools] = useState<string[]>([]);
  const [agentFormModel, setAgentFormModel] = useState("");
  const [agentFormTemp, setAgentFormTemp] = useState("");
  const [agentFormMaxIter, setAgentFormMaxIter] = useState("");
  const [agentFormMaxTokens, setAgentFormMaxTokens] = useState("");
  const [availableTools, setAvailableTools] = useState<ToolInfo[]>([]);
  const [toolFilter, setToolFilter] = useState("");
  const [agentError, setAgentError] = useState<string | null>(null);

  // ── Skill management state ──
  const [skillsOpen, setSkillsOpen] = useState(false);
  const [skills, setSkills] = useState<SkillDefinition[]>([]);
  const [skillsLoading, setSkillsLoading] = useState(false);
  const [skillFormOpen, setSkillFormOpen] = useState(false);
  const [editSkill, setEditSkill] = useState<SkillDefinition | null>(null);
  const [skillFormName, setSkillFormName] = useState("");
  const [skillFormDesc, setSkillFormDesc] = useState("");
  const [skillFormTrigger, setSkillFormTrigger] = useState("/");
  const [skillFormMode, setSkillFormMode] = useState("ask");
  const [skillFormAgent, setSkillFormAgent] = useState("");
  const [skillFormTools, setSkillFormTools] = useState("");
  const [skillFormTemplate, setSkillFormTemplate] = useState("");
  const [skillError, setSkillError] = useState<string | null>(null);

  // ── Local model health + memory ops state ──
  const [localHealth, setLocalHealth] = useState<LocalModelHealth | null>(null);
  const [healthLoading, setHealthLoading] = useState(false);
  const [memoryBusy, setMemoryBusy] = useState(false);
  const [memoryMsg, setMemoryMsg] = useState<string | null>(null);

  const checkHealth = async () => {
    setHealthLoading(true);
    const health = await checkLocalModel();
    setLocalHealth(health);
    setHealthLoading(false);
  };

  const handleCleanupMemory = async () => {
    setMemoryBusy(true);
    setMemoryMsg(null);
    const report = await cleanupMemory();
    if (report) {
      setMemoryMsg(`GC 完成: 驱逐 ${report.evicted_entries ?? 0} 条记忆, 过期 ${report.expired_entries ?? 0} 条, 删除 ${report.notes_deleted ?? 0} 个旧笔记, 清理 ${report.sessions_deleted ?? 0} 个旧会话`);
    } else {
      setMemoryMsg("GC 失败或未运行在 Tauri 环境");
    }
    setMemoryBusy(false);
  };

  const handleDeepDreaming = async () => {
    setMemoryBusy(true);
    setMemoryMsg(null);
    const result = await runDeepDreaming();
    setMemoryMsg(result ?? "Deep Dreaming 失败");
    setMemoryBusy(false);
  };

  const handleExportData = async () => {
    setMemoryBusy(true);
    setMemoryMsg(null);
    const result = await exportTrainingData();
    setMemoryMsg(result ?? "导出失败(请先在设置中启用微调数据管道)");
    setMemoryBusy(false);
  };

  const loadMcpServers = async () => {
    setMcpLoading(true);
    const servers = await listMcpServers();
    setMcpServers(servers);
    setMcpLoading(false);
  };

  const handleAddMcpServer = async () => {
    if (!newMcpName.trim() || !newMcpCommand.trim()) return;
    setMcpError(null);
    const args = newMcpArgs.split(/\s+/).filter(Boolean);
    const env = newMcpEnv.split(/\s*;\s*/).filter(Boolean);
    const result = await connectMcpServer({
      name: newMcpName.trim(),
      command: newMcpCommand.trim(),
      args,
      env,
      enabled: true,
    });
    if (result !== null) {
      setAddMcpOpen(false);
      setNewMcpName("");
      setNewMcpCommand("");
      setNewMcpArgs("");
      setNewMcpEnv("");
      await loadMcpServers();
    } else {
      setMcpError("Failed to connect MCP server");
    }
  };

  const handleDisconnectMcp = async (name: string) => {
    await disconnectMcpServer(name);
    await loadMcpServers();
  };

  // ── Agent handlers ──
  const loadAgents = async () => {
    setAgentsLoading(true);
    const list = await getAgents();
    setAgents(list);
    setAgentsLoading(false);
  };

  const startNewAgent = () => {
    setEditAgent(null);
    setAgentFormId("");
    setAgentFormName("");
    setAgentFormDesc("");
    setAgentFormPrompt("");
    setAgentFormTools([]);
    setAgentFormModel("");
    setAgentFormTemp("");
    setAgentFormMaxIter("");
    setAgentFormMaxTokens("");
    setAgentError(null);
    setAgentFormOpen(true);
  };

  const startEditAgent = (agent: AgentDefinition) => {
    setEditAgent(agent);
    setAgentFormId(agent.id);
    setAgentFormName(agent.name);
    setAgentFormDesc(agent.description);
    setAgentFormPrompt(agent.system_prompt);
    setAgentFormTools([...agent.tool_names]);
    setAgentFormModel(agent.model || "");
    setAgentFormTemp(agent.temperature != null ? String(agent.temperature) : "");
    setAgentFormMaxIter(agent.max_iterations != null ? String(agent.max_iterations) : "");
    setAgentFormMaxTokens(agent.max_tokens != null ? String(agent.max_tokens) : "");
    setAgentError(null);
    setAgentFormOpen(true);
  };

  const handleSaveAgent = async () => {
    if (!agentFormId.trim() || !agentFormName.trim()) {
      setAgentError("ID and Name are required");
      return;
    }
    setAgentError(null);
    const agent: AgentDefinition = {
      id: agentFormId.trim().toLowerCase().replace(/\s+/g, "_"),
      name: agentFormName.trim(),
      description: agentFormDesc.trim(),
      system_prompt: agentFormPrompt,
      tool_names: agentFormTools,
      model: agentFormModel.trim() || null,
      temperature: agentFormTemp ? parseFloat(agentFormTemp) : null,
      max_iterations: agentFormMaxIter ? parseInt(agentFormMaxIter) : null,
      max_tokens: agentFormMaxTokens ? parseInt(agentFormMaxTokens) : null,
    };
    const result = await saveAgent(agent);
    if (result !== null) {
      setAgentFormOpen(false);
      await loadAgents();
    } else {
      setAgentError("Failed to save agent");
    }
  };

  const handleDeleteAgent = async (agentId: string) => {
    if (!confirm(`Delete agent "${agentId}"?`)) return;
    const ok = await deleteAgent(agentId);
    if (ok) await loadAgents();
  };

  const toggleTool = (toolName: string) => {
    setAgentFormTools((prev) =>
      prev.includes(toolName)
        ? prev.filter((t) => t !== toolName)
        : [...prev, toolName]
    );
  };

  // ── Skill handlers ──
  const loadSkills = async () => {
    setSkillsLoading(true);
    const list = await listSkills();
    setSkills(list);
    setSkillsLoading(false);
  };

  const startNewSkill = () => {
    setEditSkill(null);
    setSkillFormName("");
    setSkillFormDesc("");
    setSkillFormTrigger("/");
    setSkillFormMode("ask");
    setSkillFormAgent("");
    setSkillFormTools("");
    setSkillFormTemplate("");
    setSkillError(null);
    setSkillFormOpen(true);
  };

  const startEditSkill = (skill: SkillDefinition) => {
    setEditSkill(skill);
    setSkillFormName(skill.name);
    setSkillFormDesc(skill.description);
    setSkillFormTrigger(skill.trigger);
    setSkillFormMode(skill.mode);
    setSkillFormAgent(skill.agent || "");
    setSkillFormTools(skill.tools ? skill.tools.join(", ") : "");
    setSkillFormTemplate(skill.template);
    setSkillError(null);
    setSkillFormOpen(true);
  };

  const handleSaveSkill = async () => {
    if (!skillFormTrigger.trim() || !skillFormName.trim()) {
      setSkillError("Trigger and Name are required");
      return;
    }
    setSkillError(null);
    const skill: SkillDefinition = {
      name: skillFormName.trim(),
      description: skillFormDesc.trim(),
      trigger: skillFormTrigger.trim().startsWith("/")
        ? skillFormTrigger.trim()
        : "/" + skillFormTrigger.trim(),
      mode: skillFormMode,
      agent: skillFormAgent.trim() || null,
      tools: skillFormTools
        ? skillFormTools.split(",").map((s) => s.trim()).filter(Boolean)
        : null,
      template: skillFormTemplate,
    };
    const result = await saveSkill(skill);
    if (result !== null) {
      setSkillFormOpen(false);
      await loadSkills();
    } else {
      setSkillError("Failed to save skill");
    }
  };

  const handleDeleteSkill = async (trigger: string) => {
    if (!confirm(`Delete skill "${trigger}"?`)) return;
    const ok = await deleteSkill(trigger);
    if (ok) await loadSkills();
  };

  useEffect(() => {
    async function load() {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const s = await invoke<SettingsData>("get_settings");
        setSettings(s);
        // Apply theme from saved settings
        const t = s.theme === "Light" ? "light" : "dark";
        document.documentElement.setAttribute("data-theme", t);
      } catch {
        // Running without Tauri
      }
    }
    load();
  }, []);

  // Load log path on mount
  useEffect(() => {
    getLogPath().then((p) => { if (p) setLogPath(p); });
  }, []);

  // Load logs when panel opens
  useEffect(() => {
    if (logOpen) {
      setLogLoading(true);
      getAppLogs(300).then((content) => {
        setLogContent(content);
        setLogLoading(false);
        // Scroll to bottom
        setTimeout(() => {
          if (logRef.current) {
            logRef.current.scrollTop = logRef.current.scrollHeight;
          }
        }, 50);
      });
    }
  }, [logOpen]);

  async function handleSave() {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("update_settings", { settings });
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch {
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    }
  }

  function update<K extends keyof SettingsData>(
    key: K,
    value: SettingsData[K]
  ) {
    setSettings((prev) => ({ ...prev, [key]: value }));
  }

  return (
    <div className="settings-panel">
      <h3>Settings</h3>

      <div className="settings-group">
        <label>LLM Provider</label>
        <select
          className="settings-select"
          value={settings.llm_provider}
          onChange={(e) => update("llm_provider", e.target.value)}
        >
          <option value="openai">OpenAI</option>
          <option value="anthropic">Anthropic</option>
          <option value="deepseek">DeepSeek</option>
          <option value="ollama">Ollama (Local)</option>
        </select>
      </div>

      <div className="settings-group">
        <label>Theme</label>
        <select
          className="settings-select"
          value={settings.theme}
          onChange={(e) => {
            update("theme", e.target.value);
            const t = e.target.value === "Light" ? "light" : "dark";
            document.documentElement.setAttribute("data-theme", t);
            localStorage.setItem("neecoder-theme", t);
          }}
        >
          <option value="Dark">Dark</option>
          <option value="Light">Light</option>
        </select>
      </div>

      <div className="settings-group">
        <label>API Key</label>
        <input
          className="settings-input"
          type="password"
          placeholder="sk-..."
          value={settings.api_key}
          onChange={(e) => update("api_key", e.target.value)}
        />
      </div>

      <div className="settings-group">
        <label>Completion Model</label>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <input
            className="settings-input"
            style={{ flex: 1 }}
            value={settings.completion_model}
            onChange={(e) => update("completion_model", e.target.value)}
          />
          <span style={{ fontSize: 11, color: "#8b949e", whiteSpace: "nowrap" }}>
            ctx: {modelContextWindow(settings.completion_model)}
          </span>
        </div>
      </div>

      <div className="settings-group">
        <label>Chat Model</label>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <input
            className="settings-input"
            style={{ flex: 1 }}
            value={settings.chat_model}
            onChange={(e) => update("chat_model", e.target.value)}
          />
          <span style={{ fontSize: 11, color: "#8b949e", whiteSpace: "nowrap" }}>
            ctx: {modelContextWindow(settings.chat_model)}
          </span>
        </div>
      </div>

      <div className="settings-group">
        <label>Embedding Model</label>
        <input
          className="settings-input"
          value={settings.embedding_model}
          onChange={(e) => update("embedding_model", e.target.value)}
        />
      </div>

      <div className="settings-group">
        <label>Fast Model <span className="settings-hint">(for simple tasks, summaries)</span></label>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <input
            className="settings-input"
            style={{ flex: 1 }}
            value={settings.fast_model}
            onChange={(e) => update("fast_model", e.target.value)}
            placeholder="e.g. deepseek-chat, gpt-4o-mini"
          />
          <span style={{ fontSize: 11, color: "#8b949e", whiteSpace: "nowrap" }}>
            ctx: {modelContextWindow(settings.fast_model)}
          </span>
        </div>
      </div>

      {/* ── Local Model (Ollama) ── */}
      <div className="settings-group" style={{ borderTop: "1px solid var(--border, #30363d)", paddingTop: 12, marginTop: 8 }}>
        <label style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <input
            type="checkbox"
            checked={settings.local_model.enabled}
            onChange={(e) =>
              update("local_model", { ...settings.local_model, enabled: e.target.checked })
            }
          />
          Local Model (Ollama) <span className="settings-hint">— 隐私/低成本路由, 自动降级到远程</span>
        </label>

        {settings.local_model.enabled && (
          <div style={{ marginTop: 10, display: "flex", flexDirection: "column", gap: 8 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
              <input
                className="settings-input"
                style={{ flex: 1 }}
                value={settings.local_model.base_url}
                onChange={(e) =>
                  update("local_model", { ...settings.local_model, base_url: e.target.value })
                }
                placeholder="http://localhost:11434"
              />
              <button className="settings-button" onClick={checkHealth} disabled={healthLoading}>
                {healthLoading ? "检查中…" : "检查状态"}
              </button>
            </div>
            {localHealth && (
              <div style={{ fontSize: 12, color: localHealth.running ? "#3fb950" : "#f85149" }}>
                {localHealth.running
                  ? `● 运行中 (${localHealth.models.length} 个模型: ${localHealth.models.slice(0, 5).join(", ")}${localHealth.models.length > 5 ? "…" : ""})`
                  : `● 未运行 — 将自动降级到远程 API${localHealth.error ? ` (${localHealth.error})` : ""}`}
              </div>
            )}
            <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8 }}>
              <input
                className="settings-input"
                value={settings.local_model.dreaming_model}
                onChange={(e) =>
                  update("local_model", { ...settings.local_model, dreaming_model: e.target.value })
                }
                placeholder="Dreaming 模型 (qwen2.5:3b)"
                title="记忆整理/总结用本地模型"
              />
              <input
                className="settings-input"
                value={settings.local_model.inference_model}
                onChange={(e) =>
                  update("local_model", { ...settings.local_model, inference_model: e.target.value })
                }
                placeholder="推理模型 (qwen2.5:7b)"
                title="简单对话用本地模型"
              />
            </div>
            <input
              className="settings-input"
              value={settings.local_model.embedding_model}
              onChange={(e) =>
                update("local_model", { ...settings.local_model, embedding_model: e.target.value })
              }
              placeholder="Embedding 模型 (nomic-embed-text)"
              title="记忆语义搜索用本地 embedding 模型"
            />
          </div>
        )}
      </div>

      {/* ── Memory Management ── */}
      <div className="settings-group" style={{ borderTop: "1px solid var(--border, #30363d)", paddingTop: 12, marginTop: 8 }}>
        <label>Memory Management</label>

        <label className="settings-checkbox">
          <input
            type="checkbox"
            checked={settings.memory_gc.semantic_search}
            onChange={(e) =>
              update("memory_gc", { ...settings.memory_gc, semantic_search: e.target.checked })
            }
          />
          语义搜索 <span className="settings-hint">(embedding, 需本地 Ollama embedding 模型或远程 API)</span>
        </label>

        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr", gap: 8, marginTop: 8 }}>
          <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            <span style={{ fontSize: 11, color: "#8b949e" }}>记忆 Token 上限</span>
            <input
              className="settings-input"
              type="number"
              value={settings.memory_gc.max_memory_tokens}
              onChange={(e) =>
                update("memory_gc", { ...settings.memory_gc, max_memory_tokens: Number(e.target.value) })
              }
            />
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            <span style={{ fontSize: 11, color: "#8b949e" }}>笔记保留(天)</span>
            <input
              className="settings-input"
              type="number"
              value={settings.memory_gc.notes_retention_days}
              onChange={(e) =>
                update("memory_gc", { ...settings.memory_gc, notes_retention_days: Number(e.target.value) })
              }
            />
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
            <span style={{ fontSize: 11, color: "#8b949e" }}>会话保留(天)</span>
            <input
              className="settings-input"
              type="number"
              value={settings.memory_gc.session_retention_days}
              onChange={(e) =>
                update("memory_gc", { ...settings.memory_gc, session_retention_days: Number(e.target.value) })
              }
            />
          </div>
        </div>

        <div style={{ display: "flex", gap: 8, marginTop: 10, flexWrap: "wrap" }}>
          <button className="settings-button" onClick={handleCleanupMemory} disabled={memoryBusy}>
            立即 GC
          </button>
          <button className="settings-button" onClick={handleDeepDreaming} disabled={memoryBusy}>
            Deep Dreaming
          </button>
          <button className="settings-button" onClick={handleExportData} disabled={memoryBusy}>
            导出微调数据
          </button>
        </div>
        {memoryMsg && (
          <div style={{ fontSize: 12, color: "#8b949e", marginTop: 8, whiteSpace: "pre-wrap" }}>
            {memoryMsg}
          </div>
        )}
      </div>

      {/* ── Fine-tune Pipeline ── */}
      <div className="settings-group" style={{ borderTop: "1px solid var(--border, #30363d)", paddingTop: 12, marginTop: 8 }}>
        <label style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <input
            type="checkbox"
            checked={settings.fine_tune.enabled}
            onChange={(e) =>
              update("fine_tune", { ...settings.fine_tune, enabled: e.target.checked })
            }
          />
          Fine-Tune 数据管道 <span className="settings-hint">(MEMORY.md → JSONL → LoRA)</span>
        </label>
        {settings.fine_tune.enabled && (
          <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr", gap: 8, marginTop: 8 }}>
            <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              <span style={{ fontSize: 11, color: "#8b949e" }}>LoRA Rank</span>
              <input
                className="settings-input"
                type="number"
                value={settings.fine_tune.lora_rank}
                onChange={(e) =>
                  update("fine_tune", { ...settings.fine_tune, lora_rank: Number(e.target.value) })
                }
              />
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              <span style={{ fontSize: 11, color: "#8b949e" }}>Epochs</span>
              <input
                className="settings-input"
                type="number"
                value={settings.fine_tune.epochs}
                onChange={(e) =>
                  update("fine_tune", { ...settings.fine_tune, epochs: Number(e.target.value) })
                }
              />
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              <span style={{ fontSize: 11, color: "#8b949e" }}>触发阈值(条)</span>
              <input
                className="settings-input"
                type="number"
                value={settings.fine_tune.threshold_count}
                onChange={(e) =>
                  update("fine_tune", { ...settings.fine_tune, threshold_count: Number(e.target.value) })
                }
              />
            </div>
          </div>
        )}
      </div>

      <label className="settings-checkbox">
        <input
          type="checkbox"
          checked={settings.model_routing_enabled}
          onChange={(e) => update("model_routing_enabled", e.target.checked)}
        />
        Enable Model Routing (auto-select model by task complexity)
      </label>

      {/* ── Extended Thinking (Claude) ── */}
      <div style={{ marginTop: 8, padding: "12px 16px", background: "rgba(137,180,250,0.08)", borderRadius: 6, border: "1px solid rgba(137,180,250,0.2)" }}>
        <label className="settings-checkbox" style={{ marginBottom: 8 }}>
          <input
            type="checkbox"
            checked={settings.thinking_enabled}
            onChange={(e) => update("thinking_enabled", e.target.checked)}
          />
          <span>Enable Extended Thinking (Claude only)</span>
        </label>
        {settings.thinking_enabled && (
          <div className="settings-group" style={{ marginTop: 8, marginLeft: 24 }}>
            <label style={{ fontSize: 12, color: "var(--text-muted)" }}>Thinking Budget: {settings.thinking_budget} tokens</label>
            <input
              type="range"
              min={1024}
              max={10000}
              step={256}
              value={settings.thinking_budget}
              onChange={(e) => update("thinking_budget", parseInt(e.target.value))}
              style={{ width: "100%" }}
            />
          </div>
        )}
      </div>

      <label className="settings-checkbox">
        <input
          type="checkbox"
          checked={settings.completion_enabled}
          onChange={(e) => update("completion_enabled", e.target.checked)}
        />
        Enable Code Completion
      </label>

      {/* ── Auto Review Settings ── */}
      <div style={{ marginTop: 8, padding: "12px 16px", background: "rgba(166,227,161,0.08)", borderRadius: 6, border: "1px solid rgba(166,227,161,0.2)" }}>
        <div style={{ fontWeight: 600, fontSize: 12, marginBottom: 8, color: "#a6e3a1", display: "flex", alignItems: "center", gap: 6 }}><Search size={12} /> Auto Code Review</div>
        <label className="settings-checkbox" style={{ marginBottom: 8 }}>
          <input
            type="checkbox"
            checked={settings.auto_review_on_save || false}
            onChange={(e) => update("auto_review_on_save", e.target.checked)}
          />
          Review on Save
        </label>
        <label className="settings-checkbox">
          <input
            type="checkbox"
            checked={settings.auto_review_on_commit || false}
            onChange={(e) => update("auto_review_on_commit", e.target.checked)}
          />
          Review on Commit
        </label>
      </div>

      <div className="settings-group">
        <label>Trigger Debounce (ms)</label>
        <input
          className="settings-input"
          type="number"
          value={settings.trigger_debounce_ms}
          onChange={(e) =>
            update("trigger_debounce_ms", parseInt(e.target.value) || 300)
          }
        />
      </div>

      <div className="settings-group">
        <label>Max Context Tokens</label>
        <input
          className="settings-input"
          type="number"
          value={settings.max_context_tokens}
          onChange={(e) =>
            update("max_context_tokens", parseInt(e.target.value) || 8192)
          }
        />
      </div>

      <div className="settings-group">
        <label>Custom Instructions</label>
        <textarea
          className="settings-input"
          style={{ minHeight: 80, resize: "vertical" }}
          placeholder="e.g., Always use Result type in Rust"
          value={settings.custom_instructions}
          onChange={(e) => update("custom_instructions", e.target.value)}
        />
      </div>

      <button className="btn-primary" onClick={handleSave}>
        {saved ? "Saved!" : "Save Settings"}
      </button>

      {/* ── MCP 服务器管理 ── */}
      <div style={{ marginTop: 24, borderTop: "1px solid #333", paddingTop: 16 }}>
        <div
          style={{ cursor: "pointer", display: "flex", alignItems: "center", gap: 8 }}
          onClick={() => { setMcpOpen(!mcpOpen); if (!mcpOpen) loadMcpServers(); }}
        >
          <span>{mcpOpen ? "▼" : "▶"}</span>
          <span style={{ fontWeight: 600, display: "inline-flex", alignItems: "center", gap: 6 }}><Plug size={12} /> MCP Servers</span>
          <span style={{ fontSize: 11, color: "#888" }}>
            {mcpServers.filter((s) => s.connected).length}/{mcpServers.length} connected
          </span>
        </div>
        {mcpOpen && (
          <div style={{ marginTop: 8 }}>
            {mcpError && (
              <div style={{
                padding: "6px 10px",
                background: "rgba(239, 68, 68, 0.1)",
                borderRadius: 4,
                fontSize: 12,
                color: "#ef4444",
                marginBottom: 8,
              }}>
                {mcpError}
                <button
                  onClick={() => setMcpError(null)}
                  style={{
                    background: "none", border: "none", color: "#ef4444",
                    cursor: "pointer", float: "right", fontSize: 14,
                  }}
                >✕</button>
              </div>
            )}

            {mcpLoading ? (
              <div style={{ fontSize: 12, color: "#888", padding: 8 }}>Loading...</div>
            ) : mcpServers.length === 0 ? (
              <div style={{ fontSize: 12, color: "#888", padding: 8 }}>
                No MCP servers configured. Click "Add Server" to connect one.
              </div>
            ) : (
              <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                {mcpServers.map((server) => (
                  <div
                    key={server.name}
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: 8,
                      padding: "8px 10px",
                      background: "#1a1a2e",
                      borderRadius: 6,
                      border: "1px solid #333",
                      fontSize: 12,
                    }}
                  >
                    <span
                      style={{
                        width: 8, height: 8, borderRadius: "50%",
                        background: server.connected ? "#a6e3a1" : "#f38ba8",
                        flexShrink: 0,
                      }}
                    />
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ fontWeight: 600, color: "#cdd6f4" }}>{server.name}</div>
                      <div style={{ fontSize: 10, color: "#888", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                        {server.command} {server.args.join(" ")}
                      </div>
                    </div>
                    <span style={{ fontSize: 10, color: "#888", flexShrink: 0 }}>
                      {server.connected ? `${server.tool_count} tools` : "disconnected"}
                    </span>
                    <button
                      onClick={() => handleDisconnectMcp(server.name)}
                      style={{
                        background: "none",
                        border: "1px solid #555",
                        color: "#f38ba8",
                        cursor: "pointer",
                        fontSize: 10,
                        padding: "3px 8px",
                        borderRadius: 4,
                      }}
                    >
                      Disconnect
                    </button>
                  </div>
                ))}
              </div>
            )}

            <div style={{ marginTop: 8, display: "flex", gap: 6 }}>
              <button
                className="btn-primary"
                style={{ fontSize: 11, padding: "4px 10px" }}
                onClick={loadMcpServers}
              >
                {mcpLoading ? "Loading..." : "Refresh"}
              </button>
              <button
                className="btn-primary"
                style={{ fontSize: 11, padding: "4px 10px" }}
                onClick={() => setAddMcpOpen(!addMcpOpen)}
              >
                {addMcpOpen ? "Cancel" : "Add Server"}
              </button>
            </div>

            {/* Add MCP Server form */}
            {addMcpOpen && (
              <div style={{
                marginTop: 8,
                padding: 12,
                background: "#1a1a2e",
                borderRadius: 6,
                border: "1px solid #333",
                display: "flex",
                flexDirection: "column",
                gap: 8,
              }}>
                <div className="settings-group">
                  <label>Server Name</label>
                  <input
                    className="settings-input"
                    placeholder="e.g., filesystem"
                    value={newMcpName}
                    onChange={(e) => setNewMcpName(e.target.value)}
                  />
                </div>
                <div className="settings-group">
                  <label>Command</label>
                  <input
                    className="settings-input"
                    placeholder="e.g., npx"
                    value={newMcpCommand}
                    onChange={(e) => setNewMcpCommand(e.target.value)}
                  />
                </div>
                <div className="settings-group">
                  <label>Arguments (space-separated)</label>
                  <input
                    className="settings-input"
                    placeholder="e.g., -y @anthropic/mcp-server-filesystem /path"
                    value={newMcpArgs}
                    onChange={(e) => setNewMcpArgs(e.target.value)}
                  />
                </div>
                <div className="settings-group">
                  <label>Environment (semicolon-separated, e.g., KEY=val; KEY2=val2)</label>
                  <input
                    className="settings-input"
                    placeholder="API_KEY=xxx"
                    value={newMcpEnv}
                    onChange={(e) => setNewMcpEnv(e.target.value)}
                  />
                </div>
                <button
                  className="btn-primary"
                  style={{ fontSize: 12, padding: "6px 12px" }}
                  onClick={handleAddMcpServer}
                >
                  Connect Server
                </button>
              </div>
            )}
          </div>
        )}
      </div>

      {/* ── Agent 管理 ── */}
      <div style={{ marginTop: 24, borderTop: "1px solid #333", paddingTop: 16 }}>
        <div
          style={{ cursor: "pointer", display: "flex", alignItems: "center", gap: 8 }}
          onClick={() => { setAgentsOpen(!agentsOpen); if (!agentsOpen) { loadAgents(); listAvailableTools().then(setAvailableTools); } }}
        >
          <span>{agentsOpen ? "▼" : "▶"}</span>
          <span style={{ fontWeight: 600, display: "inline-flex", alignItems: "center", gap: 6 }}><Bot size={12} /> Agents</span>
          <span style={{ fontSize: 11, color: "#888" }}>{agents.length} defined</span>
        </div>
        {agentsOpen && (
          <div style={{ marginTop: 8 }}>
            {agentError && (
              <div style={{ padding: "6px 10px", background: "rgba(239, 68, 68, 0.1)", borderRadius: 4, fontSize: 12, color: "#ef4444", marginBottom: 8 }}>
                {agentError}
                <button onClick={() => setAgentError(null)} style={{ background: "none", border: "none", color: "#ef4444", cursor: "pointer", float: "right", fontSize: 14 }}>✕</button>
              </div>
            )}

            {agentsLoading ? (
              <div style={{ fontSize: 12, color: "#888", padding: 8 }}>Loading...</div>
            ) : agents.length === 0 ? (
              <div style={{ fontSize: 12, color: "#888", padding: 8 }}>No agents defined. Create your first custom agent!</div>
            ) : (
              <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                {agents.map((agent) => (
                  <div key={agent.id} style={{ display: "flex", alignItems: "center", gap: 8, padding: "8px 10px", background: "#1a1a2e", borderRadius: 6, border: "1px solid #333", fontSize: 12 }}>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ fontWeight: 600, color: "#cdd6f4" }}>{agent.name} <span style={{ fontSize: 10, color: "#888" }}>#{agent.id}</span></div>
                      <div style={{ fontSize: 10, color: "#a6adc8", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{agent.description}</div>
                      <div style={{ fontSize: 10, color: "#888", marginTop: 2 }}>{agent.tool_names.length} tools · max {agent.max_iterations ?? "-"} iters</div>
                    </div>
                    <button onClick={() => startEditAgent(agent)} style={{ background: "none", border: "1px solid #555", color: "#89b4fa", cursor: "pointer", fontSize: 10, padding: "3px 8px", borderRadius: 4 }}>Edit</button>
                    <button onClick={() => handleDeleteAgent(agent.id)} style={{ background: "none", border: "1px solid #555", color: "#f38ba8", cursor: "pointer", fontSize: 10, padding: "3px 8px", borderRadius: 4 }}>Del</button>
                  </div>
                ))}
              </div>
            )}

            <div style={{ marginTop: 8, display: "flex", gap: 6 }}>
              <button className="btn-primary" style={{ fontSize: 11, padding: "4px 10px" }} onClick={() => { loadAgents(); listAvailableTools().then(setAvailableTools); }}>{agentsLoading ? "Loading..." : "Refresh"}</button>
              <button className="btn-primary" style={{ fontSize: 11, padding: "4px 10px" }} onClick={agentFormOpen ? () => setAgentFormOpen(false) : startNewAgent}>{agentFormOpen ? "Cancel" : "Create Agent"}</button>
            </div>

            {/* Agent Form */}
            {agentFormOpen && (
              <div style={{ marginTop: 8, padding: 12, background: "#1a1a2e", borderRadius: 6, border: "1px solid #333", display: "flex", flexDirection: "column", gap: 8 }}>
                <div className="settings-group">
                  <label>Agent ID (unique, e.g., my_reviewer)</label>
                  <input className="settings-input" value={agentFormId} onChange={(e) => setAgentFormId(e.target.value)} placeholder="my_agent" disabled={!!editAgent} />
                </div>
                <div className="settings-group">
                  <label>Display Name</label>
                  <input className="settings-input" value={agentFormName} onChange={(e) => setAgentFormName(e.target.value)} placeholder="My Custom Agent" />
                </div>
                <div className="settings-group">
                  <label>Description</label>
                  <input className="settings-input" value={agentFormDesc} onChange={(e) => setAgentFormDesc(e.target.value)} placeholder="Brief description of what this agent does" />
                </div>
                <div className="settings-group">
                  <label>System Prompt</label>
                  <textarea className="settings-input" style={{ minHeight: 100, resize: "vertical" }} value={agentFormPrompt} onChange={(e) => setAgentFormPrompt(e.target.value)} placeholder="You are a ..." />
                </div>
                <div className="settings-group">
                  <label>Model (optional, leave empty for default)</label>
                  <input className="settings-input" value={agentFormModel} onChange={(e) => setAgentFormModel(e.target.value)} placeholder="e.g., deepseek-chat" />
                </div>
                <div style={{ display: "flex", gap: 8 }}>
                  <div className="settings-group" style={{ flex: 1 }}>
                    <label>Temperature (0-2)</label>
                    <input className="settings-input" type="number" step="0.1" min="0" max="2" value={agentFormTemp} onChange={(e) => setAgentFormTemp(e.target.value)} placeholder="0.7" />
                  </div>
                  <div className="settings-group" style={{ flex: 1 }}>
                    <label>Max Iterations</label>
                    <input className="settings-input" type="number" min="1" max="50" value={agentFormMaxIter} onChange={(e) => setAgentFormMaxIter(e.target.value)} placeholder="10" />
                  </div>
                  <div className="settings-group" style={{ flex: 1 }}>
                    <label>Max Tokens</label>
                    <input className="settings-input" type="number" min="256" value={agentFormMaxTokens} onChange={(e) => setAgentFormMaxTokens(e.target.value)} placeholder="4096" />
                  </div>
                </div>

                {/* Tool Picker */}
                <div className="settings-group">
                  <label>Tools ({agentFormTools.length} selected)</label>
                  <input className="settings-input" style={{ marginBottom: 4 }} value={toolFilter} onChange={(e) => setToolFilter(e.target.value)} placeholder="Filter tools..." />
                  <div style={{ display: "flex", flexWrap: "wrap", gap: 4, maxHeight: 150, overflow: "auto", padding: "4px 0" }}>
                    {availableTools.filter((t) => !toolFilter || t.name.includes(toolFilter.toLowerCase()) || t.description.toLowerCase().includes(toolFilter.toLowerCase())).map((tool) => (
                      <label key={tool.name} style={{ display: "flex", alignItems: "center", gap: 4, fontSize: 11, padding: "3px 8px", background: agentFormTools.includes(tool.name) ? "rgba(137, 180, 250, 0.15)" : "#252536", borderRadius: 4, cursor: "pointer", border: agentFormTools.includes(tool.name) ? "1px solid #89b4fa" : "1px solid transparent" }}>
                        <input type="checkbox" checked={agentFormTools.includes(tool.name)} onChange={() => toggleTool(tool.name)} style={{ margin: 0 }} />
                        <span style={{ color: "#cdd6f4" }}>{tool.name}</span>
                        <span style={{ color: "#888", fontSize: 10 }}>{tool.description}</span>
                      </label>
                    ))}
                  </div>
                </div>

                <button className="btn-primary" style={{ fontSize: 12, padding: "6px 12px" }} onClick={handleSaveAgent}>Save Agent</button>
              </div>
            )}
          </div>
        )}
      </div>

      {/* ── Skill 管理 ── */}
      <div style={{ marginTop: 24, borderTop: "1px solid #333", paddingTop: 16 }}>
        <div
          style={{ cursor: "pointer", display: "flex", alignItems: "center", gap: 8 }}
          onClick={() => { setSkillsOpen(!skillsOpen); if (!skillsOpen) loadSkills(); }}
        >
          <span>{skillsOpen ? "▼" : "▶"}</span>
          <span style={{ fontWeight: 600, display: "inline-flex", alignItems: "center", gap: 6 }}><Zap size={12} /> Skills</span>
          <span style={{ fontSize: 11, color: "#888" }}>{skills.length} loaded</span>
        </div>
        {skillsOpen && (
          <div style={{ marginTop: 8 }}>
            {skillError && (
              <div style={{ padding: "6px 10px", background: "rgba(239, 68, 68, 0.1)", borderRadius: 4, fontSize: 12, color: "#ef4444", marginBottom: 8 }}>
                {skillError}
                <button onClick={() => setSkillError(null)} style={{ background: "none", border: "none", color: "#ef4444", cursor: "pointer", float: "right", fontSize: 14 }}>✕</button>
              </div>
            )}

            {skillsLoading ? (
              <div style={{ fontSize: 12, color: "#888", padding: 8 }}>Loading...</div>
            ) : skills.length === 0 ? (
              <div style={{ fontSize: 12, color: "#888", padding: 8 }}>No skills loaded. Create your first custom skill!</div>
            ) : (
              <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                {skills.map((skill) => (
                  <div key={skill.trigger} style={{ display: "flex", alignItems: "center", gap: 8, padding: "8px 10px", background: "#1a1a2e", borderRadius: 6, border: "1px solid #333", fontSize: 12 }}>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ fontWeight: 600, color: "#cdd6f4" }}>
                        <span style={{ color: "#89b4fa" }}>{skill.trigger}</span> — {skill.name}
                        <span style={{ fontSize: 10, padding: "1px 6px", borderRadius: 3, marginLeft: 6, background: skill.mode === "agent" ? "rgba(137,180,250,0.15)" : skill.mode === "edit" ? "rgba(249,226,175,0.15)" : "rgba(166,227,161,0.15)", color: skill.mode === "agent" ? "#89b4fa" : skill.mode === "edit" ? "#f9e2af" : "#a6e3a1" }}>{skill.mode}</span>
                      </div>
                      <div style={{ fontSize: 10, color: "#a6adc8", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{skill.description}</div>
                    </div>
                    <button onClick={() => startEditSkill(skill)} style={{ background: "none", border: "1px solid #555", color: "#89b4fa", cursor: "pointer", fontSize: 10, padding: "3px 8px", borderRadius: 4 }}>Edit</button>
                    <button onClick={() => handleDeleteSkill(skill.trigger)} style={{ background: "none", border: "1px solid #555", color: "#f38ba8", cursor: "pointer", fontSize: 10, padding: "3px 8px", borderRadius: 4 }}>Del</button>
                  </div>
                ))}
              </div>
            )}

            <div style={{ marginTop: 8, display: "flex", gap: 6 }}>
              <button className="btn-primary" style={{ fontSize: 11, padding: "4px 10px" }} onClick={loadSkills}>{skillsLoading ? "Loading..." : "Refresh"}</button>
              <button className="btn-primary" style={{ fontSize: 11, padding: "4px 10px" }} onClick={skillFormOpen ? () => setSkillFormOpen(false) : startNewSkill}>{skillFormOpen ? "Cancel" : "Create Skill"}</button>
            </div>

            {/* Skill Form */}
            {skillFormOpen && (
              <div style={{ marginTop: 8, padding: 12, background: "#1a1a2e", borderRadius: 6, border: "1px solid #333", display: "flex", flexDirection: "column", gap: 8 }}>
                <div className="settings-group">
                  <label>Trigger (must start with /)</label>
                  <input className="settings-input" value={skillFormTrigger} onChange={(e) => setSkillFormTrigger(e.target.value)} placeholder="/my-skill" />
                </div>
                <div className="settings-group">
                  <label>Name</label>
                  <input className="settings-input" value={skillFormName} onChange={(e) => setSkillFormName(e.target.value)} placeholder="My Skill" />
                </div>
                <div className="settings-group">
                  <label>Description</label>
                  <input className="settings-input" value={skillFormDesc} onChange={(e) => setSkillFormDesc(e.target.value)} placeholder="What this skill does" />
                </div>
                <div className="settings-group">
                  <label>Mode</label>
                  <select className="settings-select" value={skillFormMode} onChange={(e) => setSkillFormMode(e.target.value)}>
                    <option value="ask">Ask (Q&amp;A)</option>
                    <option value="edit">Edit (code changes)</option>
                    <option value="agent">Agent (autonomous)</option>
                  </select>
                </div>
                <div className="settings-group">
                  <label>Agent (optional, only used in agent mode)</label>
                  <input className="settings-input" value={skillFormAgent} onChange={(e) => setSkillFormAgent(e.target.value)} placeholder="e.g., code_writer" />
                </div>
                <div className="settings-group">
                  <label>Tools (comma-separated, optional)</label>
                  <input className="settings-input" value={skillFormTools} onChange={(e) => setSkillFormTools(e.target.value)} placeholder="read_file, edit, grep" />
                </div>
                <div className="settings-group">
                  <label>Template (use $SELECTION, $FILE_PATH, $LANGUAGE, etc.)</label>
                  <textarea className="settings-input" style={{ minHeight: 120, resize: "vertical", fontFamily: "monospace" }} value={skillFormTemplate} onChange={(e) => setSkillFormTemplate(e.target.value)} placeholder="Write the prompt template here..." />
                </div>
                <button className="btn-primary" style={{ fontSize: 12, padding: "6px 12px" }} onClick={handleSaveSkill}>Save Skill</button>
              </div>
            )}
          </div>
        )}
      </div>

      {/* ── 日志查看器 ── */}
      <div style={{ marginTop: 24, borderTop: "1px solid #333", paddingTop: 16 }}>
        <div
          style={{ cursor: "pointer", display: "flex", alignItems: "center", gap: 8 }}
          onClick={() => setLogOpen(!logOpen)}
        >
          <span>{logOpen ? "▼" : "▶"}</span>
          <span style={{ fontWeight: 600 }}>System Logs</span>
          {logPath && (
            <span style={{ fontSize: 11, color: "#888", marginLeft: "auto" }}>
              {logPath}
            </span>
          )}
        </div>
        {logOpen && (
          <div style={{ marginTop: 8 }}>
            <div style={{ display: "flex", gap: 8, marginBottom: 8 }}>
              <button
                className="btn-primary"
                style={{ fontSize: 11, padding: "4px 10px" }}
                onClick={async () => {
                  setLogLoading(true);
                  const content = await getAppLogs(300);
                  setLogContent(content);
                  setLogLoading(false);
                  setTimeout(() => {
                    if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
                  }, 50);
                }}
              >
                {logLoading ? "Loading..." : "Refresh"}
              </button>
              <button
                className="btn-primary"
                style={{ fontSize: 11, padding: "4px 10px" }}
                onClick={() => {
                  navigator.clipboard.writeText(logContent);
                }}
              >
                Copy All
              </button>
            </div>
            <pre
              ref={logRef}
              style={{
                background: "#1a1a2e",
                color: "#a8d8a8",
                fontSize: 11,
                lineHeight: 1.5,
                padding: 12,
                borderRadius: 6,
                maxHeight: 400,
                overflow: "auto",
                whiteSpace: "pre-wrap",
                wordBreak: "break-all",
                fontFamily: "'Cascadia Code', 'Fira Code', monospace",
                border: "1px solid #333",
              }}
            >
              {logContent || "(no logs yet)"}
            </pre>
          </div>
        )}
      </div>
    </div>
  );
}
