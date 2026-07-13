/**
 * Tauri API 抽象层 — 在 Tauri 环境和浏览器开发环境之间无缝切换
 */

// ── 类型定义 ──────────────────────────────────────────────────────────────

export interface FileTreeItem {
  name: string;
  path: string;
  is_dir: boolean;
  children?: FileTreeItem[] | null;
}

export interface AgentDefinition {
  id: string;
  name: string;
  description: string;
  system_prompt: string;
  tool_names: string[];
  model: string | null;
  temperature: number | null;
  max_iterations: number | null;
  max_tokens: number | null;
}

export interface AppSettings {
  llm_provider: string;
  completion_model: string;
  chat_model: string;
  embedding_model: string;
  api_key: string;
  completion_enabled: boolean;
  trigger_debounce_ms: number;
  max_context_tokens: number;
  max_prefix_lines: number;
  max_suffix_lines: number;
  custom_instructions: string;
  project_paths?: string[];
  theme: string;
}

export interface LSPSymbol {
  name: string;
  kind: string;
  file_path: string;
  start_line: number;
  start_column: number;
  end_line: number;
  end_column: number;
  detail: string | null;
}

export interface SkillDefinition {
  name: string;
  description: string;
  trigger: string;
  mode: string;
  agent: string | null;
  tools: string[] | null;
  template: string;
}

export interface ExecuteSkillResult {
  rendered_message: string;
  mode: string;
  agent: string | null;
}

export interface SearchResult {
  chunk: {
    id: string;
    file_path: string;
    start_line: number;
    end_line: number;
    language: string;
    chunk_type: string;
    content: string;
    summary: string;
  };
  score: number;
}

// ── Tauri invoke 封装 ─────────────────────────────────────────────────────

async function tryInvoke<T>(fn: () => Promise<T>): Promise<T | null> {
  try {
    return await fn();
  } catch {
    return null;
  }
}

// 检查是否在 Tauri 环境中
function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// ── API 函数 ──────────────────────────────────────────────────────────────

export async function getFileTree(path: string, maxDepth?: number): Promise<FileTreeItem[]> {
  if (!isTauri()) return mockFileTree(path);
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<FileTreeItem[]>("get_file_tree", { path, maxDepth: maxDepth ?? 2 });
}

export async function readFile(path: string): Promise<string | null> {
  if (!isTauri()) return "// Mock file content\nconsole.log('Hello from NeeCoder!');\n";
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<string>("read_file", { path }));
}

export async function writeFile(path: string, contents: string): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<void>("write_file", { path, content: contents }));
  return result !== null;
}

export async function openProject(path: string): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<void>("open_project", { path }));
  return result !== null;
}

export async function getSettings(): Promise<AppSettings | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<AppSettings>("get_settings"));
}

export async function updateSettings(settings: AppSettings): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<void>("update_settings", { settings }));
  return result !== null;
}

export async function getLspSymbols(language: string, filePath: string): Promise<LSPSymbol[]> {
  if (!isTauri()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<LSPSymbol[]>("get_symbols", { language, filePath }));
  return result ?? [];
}

export async function sendChatMessage(
  sessionId: string,
  message: string,
  mode: string,
  agentId?: string,
  contextFiles?: string[],
  planMode?: boolean,
  images?: string[],
  contextFolders?: string[],
  projectPath?: string,
): Promise<string | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<string>("send_message", {
    sessionId,
    message,
    mode,
    agentId: agentId || null,
    projectPath: projectPath || null,
    contextFiles: contextFiles || [],
    planMode: planMode || false,
    images: images || null,
    contextFolders: contextFolders || [],
  }));
}

export async function newSession(): Promise<string | null> {
  if (!isTauri()) return "local-session";
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<string>("new_session"));
}

export interface SessionInfo {
  id: string;
  title: string;
  message_count: number;
  created_at: string;
}

export async function getSessions(): Promise<SessionInfo[]> {
  if (!isTauri()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<SessionInfo[]>("list_sessions"));
  return result ?? [];
}

export async function deleteSession(sessionId: string): Promise<boolean> {
  if (!isTauri()) return false;
  const { invoke } = await import("@tauri-apps/api/core");
  return (await tryInvoke(() => invoke<void>("delete_session", { sessionId }))) !== null;
}

export interface SessionMessage {
  role: string;
  content: string;
}

export async function getSessionMessages(sessionId: string): Promise<SessionMessage[]> {
  if (!isTauri()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<SessionMessage[]>("get_session_messages", { sessionId }));
  return result ?? [];
}

export async function forkSession(fromSessionId: string, fromMessageIndex: number): Promise<string | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<string>("fork_session", { fromSessionId, fromMessageIndex }));
  return result ?? null;
}

export async function requestCompletion(context: {
  file_path: string;
  language: string;
  prefix: string;
  suffix: string;
  cursor_line: number;
  cursor_column: number;
}): Promise<{ id: string; text: string } | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() =>
    invoke<{ id: string; text: string }>("request_completion", { context })
  );
}

export async function startLsp(language: string, rootUri: string): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  return (await tryInvoke(() => invoke<void>("start_lsp", { language, rootUri }))) !== null;
}

export async function searchCodebase(query: string, maxResults?: number): Promise<SearchResult[]> {
  if (!isTauri()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<SearchResult[]>("search_codebase", { query, maxResults: maxResults ?? 5 }));
  return result ?? [];
}

export async function reindexProject(projectPath: string): Promise<string | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<string>("reindex_project", { projectPath }));
}

export async function answerAgentQuestion(questionId: string, answers: string[]): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<void>("answer_agent_question", { questionId, answers }));
  return result !== null;
}

export async function answerConfirm(confirmId: string, allowed: boolean): Promise<void> {
  if (!isTauri()) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await tryInvoke(() => invoke<void>("answer_confirm", { confirmId, allowed }));
}

export async function getAgents(): Promise<AgentDefinition[]> {
  if (!isTauri()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<AgentDefinition[]>("get_agents"));
  return result ?? [];
}

export async function getAppLogs(lines?: number): Promise<string> {
  if (!isTauri()) return "(not in Tauri environment)";
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<string>("get_app_logs", { lines: lines ?? 200 }));
  return result ?? "(no logs available)";
}

export async function getLogPath(): Promise<string | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<string>("get_log_path"));
}

export async function listSkills(): Promise<SkillDefinition[]> {
  if (!isTauri()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<SkillDefinition[]>("list_skills"));
  return result ?? [];
}

export async function executeSkill(params: {
  trigger: string;
  selection?: string;
  file_path?: string;
  file_content?: string;
  project_path?: string;
  arguments?: string;
}): Promise<ExecuteSkillResult | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<ExecuteSkillResult>("execute_skill", { params }));
}

export async function reloadSkills(): Promise<number | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<number>("reload_skills"));
}

// ── Agent Management ───────────────────────────────────────────────────────

export async function saveAgent(agent: AgentDefinition): Promise<string | null> {
  if (!isTauri()) return agent.id;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<string>("save_agent", { agent }));
}

export async function deleteAgent(agentId: string): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  return (await tryInvoke(() => invoke<void>("delete_agent", { agentId }))) !== null;
}

export interface ToolInfo {
  name: string;
  description: string;
  category: string;
}

export async function listAvailableTools(): Promise<ToolInfo[]> {
  if (!isTauri()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<ToolInfo[]>("list_available_tools"));
  return result ?? [];
}

// ── Skill Management ───────────────────────────────────────────────────────

export async function saveSkill(skill: SkillDefinition): Promise<string | null> {
  if (!isTauri()) return skill.trigger;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<string>("save_skill", { skill }));
}

export async function deleteSkill(trigger: string): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  return (await tryInvoke(() => invoke<void>("delete_skill", { trigger }))) !== null;
}

export interface McpServerStatus {
  name: string;
  command: string;
  args: string[];
  enabled: boolean;
  connected: boolean;
  tool_count: number;
}

// ── File operations ──

export async function createFile(path: string, content?: string): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<void>("create_file", { path, content: content || null }));
  return result !== null;
}

export async function createDirectory(path: string): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<void>("create_directory", { path }));
  return result !== null;
}

export async function deleteFileOrDir(path: string): Promise<boolean> {
  if (!isTauri()) return false;
  const { invoke } = await import("@tauri-apps/api/core");
  return (await tryInvoke(() => invoke<void>("delete_file", { path }))) !== null;
}

export async function renameFileOrDir(source: string, destination: string): Promise<boolean> {
  if (!isTauri()) return false;
  const { invoke } = await import("@tauri-apps/api/core");
  return (await tryInvoke(() => invoke<void>("rename_file", { source, destination }))) !== null;
}

// ── MCP Management ──

export async function listMcpServers(): Promise<McpServerStatus[]> {
  if (!isTauri()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<McpServerStatus[]>("list_mcp_servers"));
  return result ?? [];
}

export async function connectMcpServer(config: {
  name: string;
  command: string;
  args: string[];
  env: string[];
  enabled: boolean;
}): Promise<number | null> {
  if (!isTauri()) return 0;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<number>("connect_mcp_server", { config }));
}

export async function disconnectMcpServer(serverName: string): Promise<number | null> {
  if (!isTauri()) return 0;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<number>("disconnect_mcp_server", { serverName }));
}

// ── Cloud Agent ──

export type CloudTaskStatus = "pending" | "running" | "completed" | "failed" | "cancelled";

export interface CloudTask {
  id: string;
  session_id: string;
  status: CloudTaskStatus;
  message: string;
  created_at: number;
  completed_at: number | null;
  result: string | null;
  pr_url: string | null;
}

export async function startCloudAgent(
  message: string,
  sessionId: string,
  agentId?: string,
): Promise<string | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<string>("start_cloud_agent", {
    message,
    session_id: sessionId,
    agent_id: agentId || null,
    pr_config: null,
  }));
}

export async function getCloudTask(taskId: string): Promise<CloudTask | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<CloudTask>("get_cloud_task", { taskId }));
}

export async function listCloudTasks(): Promise<CloudTask[]> {
  if (!isTauri()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<CloudTask[]>("list_cloud_tasks"));
  return result ?? [];
}

export async function cancelCloudTask(taskId: string): Promise<boolean> {
  if (!isTauri()) return false;
  const { invoke } = await import("@tauri-apps/api/core");
  return (await tryInvoke(() => invoke<void>("cancel_cloud_task", { taskId }))) !== null;
}

// ── 事件监听 ──────────────────────────────────────────────────────────────

export type UnlistenFn = () => void;

export async function listenToEvent<T = any>(
  event: string,
  handler: (payload: T) => void
): Promise<UnlistenFn | null> {
  if (!isTauri()) return () => {};
  try {
    const { listen } = await import("@tauri-apps/api/event");
    return await listen<T>(event, (e) => handler(e.payload as T));
  } catch {
    return () => {};
  }
}

// ── Mock 文件树（开发模式） ─────────────────────────────────────────────────

function mockFileTree(rootName: string): FileTreeItem[] {
  return [
    {
      name: "src",
      path: rootName + "/src",
      is_dir: true,
      children: [
        {
          name: "components",
          path: rootName + "/src/components",
          is_dir: true,
          children: [
            { name: "App.tsx", path: rootName + "/src/App.tsx", is_dir: false },
            { name: "main.tsx", path: rootName + "/src/main.tsx", is_dir: false },
          ],
        },
        { name: "styles", path: rootName + "/src/styles", is_dir: true, children: [
          { name: "global.css", path: rootName + "/src/styles/global.css", is_dir: false },
        ]},
      ],
    },
    {
      name: "src-tauri",
      path: rootName + "/src-tauri",
      is_dir: true,
      children: [
        {
          name: "src",
          path: rootName + "/src-tauri/src",
          is_dir: true,
          children: [
            { name: "main.rs", path: rootName + "/src-tauri/src/main.rs", is_dir: false },
            { name: "lib.rs", path: rootName + "/src-tauri/src/lib.rs", is_dir: false },
          ],
        },
        { name: "Cargo.toml", path: rootName + "/Cargo.toml", is_dir: false },
      ],
    },
    { name: "package.json", path: rootName + "/package.json", is_dir: false },
    { name: "tsconfig.json", path: rootName + "/tsconfig.json", is_dir: false },
  ];
}

// ── 工具函数 ─────────────────────────────────────────────────────────────────

export interface FlatFileItem {
  name: string;
  path: string;
  is_dir?: boolean;
}

/**
 * Flatten a FileTreeItem tree into a flat list of files AND directories.
 * Directories are included so they can be referenced in @mentions and
 * sent as context_folders to the backend for expansion.
 */
export function flattenFileTree(tree: FileTreeItem[]): FlatFileItem[] {
  const result: FlatFileItem[] = [];
  function walk(items: FileTreeItem[]) {
    for (const item of items) {
      result.push({ name: item.name, path: item.path, is_dir: item.is_dir });
      if (item.children) {
        walk(item.children);
      }
    }
  }
  walk(tree);
  return result;
}
