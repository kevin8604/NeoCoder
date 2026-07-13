use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use crate::agent::definition::{find_agent, AgentRegistry};
use crate::agent::AgentInstance;
use crate::llm;

/// 执行一个子 Agent
/// 所有参数均为 owned 类型，支持 tokio::spawn 并行调度。
/// 直接调用 agent.run_no_dispatch() 避免异步递归类型循环。
/// run_no_dispatch 不支持 dispatch_agent/dispatch_agents，因此不会再次
/// 调用 run_sub_agent，彻底切断类型层面的循环依赖。
pub async fn run_sub_agent(
    app: tauri::AppHandle,
    session_id: String,
    task: String,
    agent_id: String,
    registry: AgentRegistry,
    provider: crate::config::LlmProvider,
    api_key: String,
    base_url: Option<String>,
    chat_model: String,
    project_path: Option<String>,
) -> String {
    let agent_def = find_agent(&registry, &agent_id);
    let cancelled = Arc::new(AtomicBool::new(false));

    let messages = vec![
        llm::ChatMessage {
            role: "user".into(),
            content: task,
            images: None,
            tool_calls: None,
            tool_call_id: None,
        }
    ];

    let mut agent = AgentInstance::new(
        app,
        session_id,
        messages,
        provider,
        api_key,
        base_url,
        chat_model,
        project_path,
        None,
        cancelled,
        agent_def.as_ref(),
        None, // sub-agents don't get memory context
    );

    // 直接调用 run_no_dispatch —— 不产生 Send 约束，不形成类型循环
    match agent.run_no_dispatch().await {
        Ok(text) => format!("[SUB_AGENT_RESULT:{}]\n{}", agent_id, text),
        Err(e) => format!("[SUB_AGENT_ERROR:{}]\n{}", agent_id, e),
    }
}

/// 执行多个子 Agent（真正并行），支持冲突检测和依赖管理
/// 使用 tokio::task::JoinSet 实现真正的并行调度。
/// 每个子 Agent 使用 run_no_dispatch 以避免异步递归类型循环。
/// tasks: (agent_id, task, file_path, depends_on)
pub async fn run_sub_agents_parallel(
    app: &tauri::AppHandle,
    session_id: &str,
    tasks: &[(String, String, Option<String>, Option<Vec<String>>)], // (agent_id, task, file_path, depends_on)
    registry: &crate::agent::definition::AgentRegistry,
    provider: &crate::config::LlmProvider,
    api_key: &str,
    base_url: Option<&str>,
    chat_model: &str,
    project_path: Option<&str>,
) -> Vec<String> {
    // ── Dependency-aware execution ──
    let has_deps = tasks.iter().any(|(_, _, _, deps)| deps.as_ref().map_or(false, |d| !d.is_empty()));

    if !has_deps {
        // Fast path: no dependencies, run all in parallel
        return run_all_parallel(app, session_id, tasks, registry, provider, api_key, base_url, chat_model, project_path).await;
    }

    // Build agent_id → output map for completed agents
    let mut completed: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut results: Vec<String> = Vec::with_capacity(tasks.len());
    let mut remaining: Vec<usize> = (0..tasks.len()).collect();

    // Iteratively find tasks whose dependencies are all satisfied
    while !remaining.is_empty() {
        let mut ready: Vec<usize> = Vec::new();
        let mut still_waiting: Vec<usize> = Vec::new();

        for &idx in &remaining {
            let (_, _, _, ref deps) = tasks[idx];
            let all_deps_met = deps.as_ref().map_or(true, |deps_list| {
                deps_list.iter().all(|dep_id| completed.contains_key(dep_id.as_str()))
            });
            if all_deps_met {
                ready.push(idx);
            } else {
                still_waiting.push(idx);
            }
        }

        if ready.is_empty() {
            log::warn!("Dependency cycle detected: {} tasks stuck", still_waiting.len());
            break;
        }

        // Build ready tasks with dependency context injected
        let mut ready_tasks: Vec<(String, String, Option<String>, Option<Vec<String>>)> = Vec::with_capacity(ready.len());
        for &idx in &ready {
            let agent_id = tasks[idx].0.clone();
            let file_path = tasks[idx].2.clone();
            let deps = tasks[idx].3.clone();
            let mut enhanced_task = tasks[idx].1.clone();
            if let Some(ref dep_list) = deps {
                if !dep_list.is_empty() {
                    enhanced_task.push_str("\n\n--- Context from dependencies ---\n");
                    for dep_id in dep_list {
                        if let Some(output) = completed.get(dep_id.as_str()) {
                            // Truncate each dependency output to 2000 chars to avoid context overflow
                            let truncated = if output.len() > 2000 {
                                format!("{}... [truncated]", crate::agent::utils::safe_truncate(output, 2000))
                            } else {
                                output.clone()
                            };
                            enhanced_task.push_str(&format!(
                                "\n[Result from agent '{}']:\n{}\n", dep_id, truncated
                            ));
                        }
                    }
                    enhanced_task.push_str("--- End dependency context ---\n");
                }
            }
            ready_tasks.push((agent_id, enhanced_task, file_path, deps));
        }

        // Run ready tasks in parallel
        let level_results = run_all_parallel(app, session_id, &ready_tasks, registry, provider, api_key, base_url, chat_model, project_path).await;

        // Store results keyed by agent_id for dependency injection
        for (i, &idx) in ready.iter().enumerate() {
            let agent_id = &tasks[idx].0;
            completed.insert(agent_id.clone(), level_results[i].clone());
            results.push(level_results[i].clone());
        }

        remaining = still_waiting;
    }

    results
}

/// Run all tasks in parallel (no dependency handling).
async fn run_all_parallel(
    app: &tauri::AppHandle,
    session_id: &str,
    tasks: &[(String, String, Option<String>, Option<Vec<String>>)],
    registry: &crate::agent::definition::AgentRegistry,
    provider: &crate::config::LlmProvider,
    api_key: &str,
    base_url: Option<&str>,
    chat_model: &str,
    project_path: Option<&str>,
) -> Vec<String> {
    // Conflict detection
    let mut seen: std::collections::HashMap<&str, Vec<usize>> = std::collections::HashMap::new();
    for (i, (_, _, fp, _)) in tasks.iter().enumerate() {
        if let Some(path) = fp.as_deref() {
            seen.entry(path).or_default().push(i);
        }
    }
    for (path, indices) in &seen {
        if indices.len() > 1 {
            log::warn!(
                "Parallel dispatch conflict: file '{}' targeted by {} agents (indices {:?})",
                path, indices.len(), indices
            );
        }
    }

    // 单任务直接执行，无需 JoinSet 开销
    if tasks.len() == 1 {
        let (agent_id, task, _fp, _deps) = &tasks[0];
        return vec![run_sub_agent(
            app.clone(),
            session_id.to_string(),
            task.clone(),
            agent_id.clone(),
            registry.clone(),
            provider.clone(),
            api_key.to_string(),
            base_url.map(|s| s.to_string()),
            chat_model.to_string(),
            project_path.map(|s| s.to_string()),
        ).await];
    }

    // 使用 JoinSet 实现真正的并行执行
    let mut join_set = tokio::task::JoinSet::new();

    for (agent_id, task, _fp, _deps) in tasks {
        join_set.spawn(run_sub_agent(
            app.clone(),
            session_id.to_string(),
            task.clone(),
            agent_id.clone(),
            registry.clone(),
            provider.clone(),
            api_key.to_string(),
            base_url.map(|s| s.to_string()),
            chat_model.to_string(),
            project_path.map(|s| s.to_string()),
        ));
    }

    // 按完成顺序收集结果
    let mut results = Vec::with_capacity(tasks.len());
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok(result) => results.push(result),
            Err(e) => results.push(format!("[SUB_AGENT_ERROR:join]\n{}", e)),
        }
    }
    results
}
