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
  auto_review_on_save?: boolean;
  auto_review_on_commit?: boolean;
  // ── A2A (Agent-to-Agent) ──
  a2a_server_enabled?: boolean;
  a2a_server_port?: number;
  a2a_server_token?: string;
  a2a_agents?: A2aAgentConfig[];
}

/** Remote A2A agent entry persisted in AppSettings. */
export interface A2aAgentConfig {
  name: string;
  url: string;
  description: string;
}

/** A2A server status as reported by get_a2a_status. */
export interface A2aStatus {
  enabled: boolean;
  running: boolean;
  port: number;
  token_set: boolean;
}

/** Agent Card (A2A v1.0, camelCase wire fields). */
export interface AgentCardSkill {
  id: string;
  name: string;
  description: string;
  tags: string[];
}

export interface AgentCard {
  name: string;
  description: string;
  url: string;
  version: string;
  capabilities?: {
    streaming?: boolean;
    pushNotifications?: boolean;
    stateTransitionHistory?: boolean;
  };
  authentication?: { schemes: string[] } | null;
  defaultInputModes?: string[];
  defaultOutputModes?: string[];
  skills?: AgentCardSkill[];
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
  if (!isTauri()) return "// Mock file content\nconsole.log('Hello from NeoCoder!');\n";
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<string>("read_file", { path }));
}

export async function writeFile(path: string, contents: string): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<void>("write_file", { path, content: contents }));
  return result !== null;
}

/** A registered workspace in the multi-workspace runtime. */
export interface Workspace {
  id: string;
  name: string;
  path: string;
  created_at: number;
  last_opened_at: number;
  index_db_path: string;
}

/**
 * Open (create-or-activate) a workspace directory.
 * Returns the activated workspace, or null on failure.
 */
export async function openProject(path: string): Promise<Workspace | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<Workspace>("open_project", { path }));
}

/** List all registered workspaces, most recently opened first. */
export async function listWorkspaces(): Promise<Workspace[]> {
  if (!isTauri()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<Workspace[]>("list_workspaces"));
  return result ?? [];
}

/** Activate a registered workspace (swaps watcher / index / project skills). */
export async function activateWorkspace(workspaceId: string): Promise<Workspace | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<Workspace>("activate_workspace", { workspaceId }));
}

/** Remove a workspace entry (and its per-workspace index DB). */
export async function removeWorkspace(workspaceId: string): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<void>("remove_workspace", { workspaceId }));
  return result !== null;
}

/** Rename a workspace entry. */
export async function renameWorkspace(workspaceId: string, newName: string): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<void>("rename_workspace", { workspaceId, newName }));
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
}): Promise<{ id: string; text: string; candidates_count: number } | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() =>
    invoke<{ id: string; text: string; candidates_count: number }>("request_completion", { context })
  );
}

export async function cycleCompletion(id: string, direction: number): Promise<string | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<string>("cycle_completion", { id, direction }));
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

export interface DependencyGraphData {
  mermaid: string;
  node_count: number;
  edge_count: number;
}

export async function getDependencyGraph(projectPath: string, depth?: number): Promise<DependencyGraphData | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<DependencyGraphData>("get_dependency_graph", { projectPath, depth: depth ?? 3 }));
}

// ── Auto Review ──

export async function triggerAutoReview(projectPath: string): Promise<{ sessionId: string; prompt: string } | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<string>("trigger_auto_review", { projectPath }));
  if (!result) return null;
  const [sessionId, prompt] = result.split("|||");
  return { sessionId, prompt };
}

export interface AutoReviewSettings {
  on_save: boolean;
  on_commit: boolean;
}

export async function getAutoReviewSettings(): Promise<AutoReviewSettings | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<AutoReviewSettings>("get_auto_review_settings"));
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

export async function approvePlan(sessionId: string): Promise<void> {
  if (!isTauri()) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await tryInvoke(() => invoke<void>("approve_plan", { sessionId }));
}

export async function rejectPlan(sessionId: string, reason?: string): Promise<void> {
  if (!isTauri()) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await tryInvoke(() => invoke<void>("reject_plan", { sessionId, reason }));
}

export async function skipPlan(sessionId: string): Promise<void> {
  if (!isTauri()) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await tryInvoke(() => invoke<void>("skip_plan", { sessionId }));
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

export type CloudTaskStatus = "pending" | "running" | "completed" | "failed" | "cancelled" | "interrupted";

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

export async function resumeCloudTask(taskId: string): Promise<boolean> {
  if (!isTauri()) return false;
  const { invoke } = await import("@tauri-apps/api/core");
  return (await tryInvoke(() => invoke<void>("resume_cloud_task", { taskId }))) !== null;
}

// ── Checkpoints (agent iteration snapshots + diff) ──

export interface Checkpoint {
  iteration: number;
  timestamp: number;
  commit_hash: string | null;
  files: string[];
  description: string;
}

export interface DiffHunk {
  type: "add" | "remove" | "context" | "hunk";
  content: string;
  old_start: number;
  new_start: number;
}

export interface FileChange {
  file_path: string;
  hunks: DiffHunk[];
}

export async function listCheckpoints(sessionId: string): Promise<Checkpoint[]> {
  if (!isTauri()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<Checkpoint[]>("list_checkpoints", { sessionId }));
  return result ?? [];
}

export async function checkpointDiff(
  sessionId: string,
  iteration: number,
  projectPath: string
): Promise<FileChange[]> {
  if (!isTauri()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() =>
    invoke<FileChange[]>("checkpoint_diff", { sessionId, iteration, projectPath })
  );
  return result ?? [];
}

export async function restoreCheckpoint(
  sessionId: string,
  iteration: number,
  projectPath: string
): Promise<boolean> {
  if (!isTauri()) return false;
  const { invoke } = await import("@tauri-apps/api/core");
  return (await tryInvoke(() =>
    invoke<void>("restore_checkpoint", { sessionId, iteration, projectPath })
  )) !== null;
}

// ── A2A (Agent-to-Agent) ──

export async function getA2aStatus(): Promise<A2aStatus | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<A2aStatus>("get_a2a_status"));
}

/** Update A2A config — all fields optional, only provided ones change. */
export async function setA2aConfig(params: {
  enabled?: boolean;
  port?: number;
  token?: string;
  agents?: A2aAgentConfig[];
}): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  return (await tryInvoke(() => invoke<void>("set_a2a_config", { params }))) !== null;
}

export async function listRemoteAgents(): Promise<A2aAgentConfig[]> {
  if (!isTauri()) return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const result = await tryInvoke(() => invoke<A2aAgentConfig[]>("list_remote_agents"));
  return result ?? [];
}

/** Discover a remote agent's Agent Card (summary shown in the UI). */
export async function discoverRemoteAgent(
  url: string,
  token?: string,
): Promise<AgentCard | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() =>
    invoke<AgentCard>("discover_remote_agent", {
      url,
      token: token || null,
    })
  );
}

/** Manually invoke a remote agent (debug / manual trigger). */
export async function invokeRemoteAgent(
  url: string,
  task: string,
  mode?: string,
  timeoutSecs?: number,
  token?: string,
  skill?: string,
): Promise<string | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() =>
    invoke<string>("invoke_remote_agent", {
      url,
      task,
      mode: mode || "sync",
      timeoutSecs: timeoutSecs ?? 120,
      token: token || null,
      skill: skill || null,
    })
  );
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
  /** Optional line number for `@file:line` references */
  line?: number;
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

// ── 本地模型 & 记忆管理 API ────────────────────────────────────────────────

export interface LocalModelHealth {
  running: boolean;
  models: string[];
  error: string | null;
}

export interface MemSearchResult {
  file_path: string;
  line_number: number;
  line_content: string;
  relevance: number;
}

/** One memory entry with its current Ebbinghaus retention (R) value. */
export interface MemEntry {
  id: string;
  text: string;
  section: string;
  category: string;
  stability: number;
  recall_count: number;
  created: string;
  last_recalled: string;
  retention: number;
}

/** Probe the local Ollama service (running state + model list). */
export async function checkLocalModel(): Promise<LocalModelHealth | null> {
  if (!isTauri()) return { running: false, models: [], error: null };
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<LocalModelHealth>("check_local_model"));
}

/** Search memory (BM25 or hybrid BM25 + embedding). */
export async function searchMemory(
  query: string,
  maxResults?: number,
  useSemantic?: boolean
): Promise<MemSearchResult[] | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() =>
    invoke<MemSearchResult[]>("search_memory", {
      query,
      maxResults: maxResults ?? 8,
      useSemantic: useSemantic ?? false,
    })
  );
}

/** Aggregate memory statistics. */
export async function getMemoryStats(): Promise<Record<string, unknown> | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<Record<string, unknown>>("get_memory_stats"));
}

/** Read memory entries with current retention (R) values for visualization. */
export async function getMemoryEntries(limit?: number): Promise<MemEntry[] | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<MemEntry[]>("get_memory_entries", { limit: limit ?? 200 }));
}

/** Run memory GC (capacity + expiration). */
export async function cleanupMemory(): Promise<Record<string, unknown> | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<Record<string, unknown>>("cleanup_memory"));
}

/** Deep Dreaming: global consolidation & dedup. */
export async function runDeepDreaming(): Promise<string | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<string>("run_deep_dreaming"));
}

/** Export MEMORY.md as JSONL fine-tune dataset. */
export async function exportTrainingData(): Promise<string | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<string>("export_training_data"));
}

// ── 记忆面板 API（A1）───────────────────────────────────────────────────────

/** Full MEMORY.md preview + stats for the MemoryPanel. */
export async function previewMemory(): Promise<{ long_term: string; stats: Record<string, unknown> } | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<{ long_term: string; stats: Record<string, unknown> }>("preview_memory"));
}

/** List daily note dates, newest first: [{date, chars, preview}]. */
export async function listNotes(): Promise<{ date: string; chars: number; preview: string }[] | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<{ date: string; chars: number; preview: string }[]>("list_notes"));
}

/** Read a daily note for a specific date (YYYY-MM-DD). */
export async function readNote(date: string): Promise<string | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<string>("read_note", { date }));
}

// ── Agent 暂停/恢复 API（C1）─────────────────────────────────────────────────

/** Pause a running agent session. */
export async function pauseAgent(sessionId: string): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<void>("pause_agent", { sessionId })) !== null;
}

/** Resume a paused agent session. */
export async function resumeAgent(sessionId: string): Promise<boolean> {
  if (!isTauri()) return true;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<void>("resume_agent", { sessionId })) !== null;
}

// ── LSP 编辑 API（B1）───────────────────────────────────────────────────────

export interface LSPTextEdit {
  file_path: string;
  start_line: number;
  start_column: number;
  end_line: number;
  end_column: number;
  new_text: string;
}

export interface LSPCodeAction {
  title: string;
  kind?: string;
  is_preferred?: boolean;
  edits?: LSPTextEdit[];
  command?: string;
}

/** Rename a symbol across the workspace via LSP. */
export async function renameSymbol(
  language: string,
  filePath: string,
  line: number,
  column: number,
  newName: string
): Promise<LSPTextEdit[] | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<LSPTextEdit[]>("rename_symbol", { language, filePath, line, column, newName }));
}

/** Format the whole document via LSP. */
export async function formatDocument(language: string, filePath: string): Promise<LSPTextEdit[] | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<LSPTextEdit[]>("format_document", { language, filePath }));
}

/** Request quick fixes / code actions at a position. */
export async function getCodeActions(
  language: string,
  filePath: string,
  line: number,
  column: number,
  diagnostics: unknown[] = []
): Promise<LSPCodeAction[] | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<LSPCodeAction[]>("get_code_actions", { language, filePath, line, column, diagnostics }));
}

// ── 遥测 API（B3）───────────────────────────────────────────────────────────

export interface TelemetrySummary {
  total_sessions: number;
  successful_sessions: number;
  failed_sessions: number;
  cancelled_sessions: number;
  success_rate: number;
  avg_iterations: number;
  total_iterations: number;
  total_tool_calls: number;
  total_tool_failures: number;
  tool_failure_rate: number;
  total_prompt_tokens: number;
  total_completion_tokens: number;
  total_completions: number;
  total_inline_edits: number;
  top_tools: [string, number][];
  model_usage: Record<string, number>;
}

export type TelemetryEventPayload = Record<string, unknown> & { type?: string };

/** Aggregate telemetry summary for the insights dashboard. */
export async function getTelemetrySummary(): Promise<TelemetrySummary | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<TelemetrySummary>("get_telemetry_summary"));
}

/** Recent telemetry events (tagged JSON). */
export async function getTelemetryEvents(count?: number): Promise<TelemetryEventPayload[] | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<TelemetryEventPayload[]>("get_telemetry_events", { count: count ?? 100 }));
}

// ── Agent 执行日志 replay（B2）──────────────────────────────────────────────

export interface AgentLogEntry {
  seq: number;
  timestamp: number;
  agent_id: string;
  type: string;
  [key: string]: unknown;
}

/** Replay the persisted JSONL execution log for a session. */
export async function replaySession(sessionId: string): Promise<AgentLogEntry[] | null> {
  if (!isTauri()) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return tryInvoke(() => invoke<AgentLogEntry[]>("replay_session", { sessionId }));
}
