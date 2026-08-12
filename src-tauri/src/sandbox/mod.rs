use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(test)]
mod tests;

// ── Sandbox Mode ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    /// Only project paths + allowed_paths; writes require path check
    Strict,
    /// Reads allowed anywhere; writes restricted to project paths
    Permissive,
    /// No sandboxing (current behaviour)
    Disabled,
}

impl Default for SandboxMode {
    fn default() -> Self {
        SandboxMode::Strict
    }
}

// ── Sandbox Config ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    pub mode: SandboxMode,
    /// Extra allowed directories (beyond project paths)
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    /// Explicitly blocked paths (highest priority)
    #[serde(default)]
    pub blocked_paths: Vec<String>,
    /// Extra blocked terminal commands (merged with built-in blacklist)
    #[serde(default)]
    pub blocked_commands: Vec<String>,
    /// Max file size for read/write in MB (0 = unlimited)
    #[serde(default = "default_max_file_size_mb")]
    pub max_file_size_mb: u32,
    /// Allowed domains for web tools (empty = all allowed)
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Fine-grained permission rules
    #[serde(default)]
    pub permissions: PermissionRules,
}

fn default_max_file_size_mb() -> u32 {
    50
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            mode: SandboxMode::Strict,
            allowed_paths: Vec::new(),
            blocked_paths: Vec::new(),
            blocked_commands: Vec::new(),
            max_file_size_mb: default_max_file_size_mb(),
            allowed_domains: Vec::new(),
            permissions: PermissionRules::default(),
        }
    }
}

// ── Permission Rules ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionRules {
    /// Paths that always require user confirmation before write
    #[serde(default)]
    pub confirm_write_paths: Vec<String>,
    /// Paths that are always allowed without sandbox check
    #[serde(default)]
    pub auto_allow_paths: Vec<String>,
    /// Commands that always require user confirmation
    #[serde(default)]
    pub confirm_commands: Vec<String>,
    /// Commands that are always allowed (bypass dangerous check)
    #[serde(default)]
    pub auto_allow_commands: Vec<String>,
}

// ── Audit Record ──

#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub timestamp: String,
    pub action: String,
    pub tool: String,
    pub target: String,
    pub reason: String,
}

impl AuditRecord {
    pub fn to_log_line(&self) -> String {
        format!(
            "[{}] action={} tool={} target=\"{}\" reason=\"{}\"",
            self.timestamp, self.action, self.tool, self.target, self.reason
        )
    }
}

// ── Sandbox Checker ──

/// Centralized security checker for all tool operations.
/// Thread-safe via internal Mutex for audit log writes.
pub struct SandboxChecker {
    pub config: SandboxConfig,
    audit_log_path: Option<PathBuf>,
    audit_records: Mutex<Vec<AuditRecord>>,
}

impl SandboxChecker {
    pub fn new(config: SandboxConfig, audit_log_path: Option<PathBuf>) -> Self {
        Self {
            config,
            audit_log_path,
            audit_records: Mutex::new(Vec::new()),
        }
    }

    // ── Path checking ──

    /// Validate that a path is within allowed boundaries.
    /// Returns the canonicalized path on success, or an error message.
    pub fn check_path(
        &self,
        path: &Path,
        project_path: Option<&str>,
        is_write: bool,
    ) -> Result<PathBuf, String> {
        // Disabled mode: pass through
        if self.config.mode == SandboxMode::Disabled {
            return Ok(path.to_path_buf());
        }

        // Canonicalize the target path for comparison
        // If the path doesn't exist yet (for writes), canonicalize the parent
        let canonical = self.canonicalize_safe(path);

        // Check blocked_paths (highest priority)
        for blocked in &self.config.blocked_paths {
            let blocked_canonical = self.canonicalize_safe(Path::new(blocked));
            if canonical.starts_with(&blocked_canonical) {
                let reason = format!(
                    "Path '{}' is in blocked paths ('{}')",
                    path.display(),
                    blocked
                );
                self.record_audit("deny", "path_check", &path.display().to_string(), &reason);
                return Err(reason);
            }
        }

        // Check auto_allow_paths (bypass all sandbox checks)
        for allowed in &self.config.permissions.auto_allow_paths {
            let allowed_canonical = self.canonicalize_safe(Path::new(allowed));
            if canonical.starts_with(&allowed_canonical) {
                return Ok(canonical);
            }
        }

        // Check confirm_write_paths (require user confirmation)
        if is_write {
            for confirm in &self.config.permissions.confirm_write_paths {
                let confirm_canonical = self.canonicalize_safe(Path::new(confirm));
                if canonical.starts_with(&confirm_canonical) {
                    let reason = format!(
                        "Path '{}' requires user confirmation before writing",
                        path.display()
                    );
                    self.record_audit("confirm", "path_check", &path.display().to_string(), &reason);
                    return Err(format!("[CONFIRM_REQUIRED] {}", reason));
                }
            }
        }

        // Build list of allowed roots
        let mut allowed_roots: Vec<PathBuf> = Vec::new();
        if let Some(pp) = project_path {
            let pp_path = Path::new(pp);
            if pp_path.exists() {
                allowed_roots.push(self.canonicalize_safe(pp_path));
            } else {
                // Project path doesn't exist yet, use as-is
                allowed_roots.push(pp_path.to_path_buf());
            }
        }
        for ap in &self.config.allowed_paths {
            let ap_path = Path::new(ap);
            if ap_path.exists() {
                allowed_roots.push(self.canonicalize_safe(ap_path));
            } else {
                allowed_roots.push(ap_path.to_path_buf());
            }
        }

        match self.config.mode {
            SandboxMode::Strict => {
                // Path must be under one of the allowed roots
                if allowed_roots.is_empty() {
                    // No project path and no allowed paths — block writes, but allow reads.
                    // Rationale: read-only agents (e.g. reviewer) may be invoked on files
                    // the user has opened in the editor even without a formal project path.
                    // Writes remain blocked until a project is configured.
                    if !is_write {
                        return Ok(canonical);
                    }
                    let reason = "No project path configured; sandbox blocks writes in strict mode, open a project first".to_string();
                    self.record_audit("deny", "path_check", &path.display().to_string(), &reason);
                    return Err(reason);
                }
                for root in &allowed_roots {
                    if canonical.starts_with(root) {
                        return Ok(canonical);
                    }
                }
                let reason = format!(
                    "Path '{}' is outside allowed directories in strict mode",
                    path.display()
                );
                self.record_audit("deny", "path_check", &path.display().to_string(), &reason);
                Err(reason)
            }
            SandboxMode::Permissive => {
                if is_write {
                    // Writes must be within allowed roots
                    if allowed_roots.is_empty() {
                        let reason = "No project path configured; sandbox blocks all writes in permissive mode".to_string();
                        self.record_audit("deny", "path_check", &path.display().to_string(), &reason);
                        return Err(reason);
                    }
                    for root in &allowed_roots {
                        if canonical.starts_with(root) {
                            return Ok(canonical);
                        }
                    }
                    let reason = format!(
                        "Write to '{}' is outside project directory in permissive mode",
                        path.display()
                    );
                    self.record_audit("deny", "path_check", &path.display().to_string(), &reason);
                    Err(reason)
                } else {
                    // Reads allowed anywhere
                    Ok(canonical)
                }
            }
            SandboxMode::Disabled => Ok(canonical),
        }
    }

    /// Safely canonicalize a path. If the path doesn't exist, canonicalize
    /// the longest existing ancestor and append the remaining components.
    fn canonicalize_safe(&self, path: &Path) -> PathBuf {
        match std::fs::canonicalize(path) {
            Ok(p) => p,
            Err(_) => {
                // Path doesn't exist — walk up until we find an existing ancestor,
                // canonicalize it, then re-append the missing tail. Checking only the
                // immediate parent would return a non-canonical path for deeply nested
                // targets, breaking starts_with comparisons against canonicalized
                // allowed roots (Windows canonical paths carry the \\?\ prefix).
                let mut missing: Vec<std::ffi::OsString> = Vec::new();
                let mut cur = path;
                loop {
                    match cur.parent() {
                        Some(parent) => {
                            if let Some(name) = cur.file_name() {
                                missing.push(name.to_os_string());
                            } else {
                                break;
                            }
                            if parent.exists() {
                                if let Ok(canonical_parent) = std::fs::canonicalize(parent) {
                                    let mut result = canonical_parent;
                                    for seg in missing.iter().rev() {
                                        result.push(seg);
                                    }
                                    return result;
                                }
                            }
                            cur = parent;
                        }
                        None => break,
                    }
                }
                // Fallback: return absolute path
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    std::env::current_dir()
                        .unwrap_or_default()
                        .join(path)
                }
            }
        }
    }

    // ── File size checking ──

    pub fn check_file_size(&self, path: &Path) -> Result<(), String> {
        if self.config.max_file_size_mb == 0 {
            return Ok(()); // 0 = unlimited
        }
        if path.exists() {
            match std::fs::metadata(path) {
                Ok(meta) => {
                    let size_mb = meta.len() / (1024 * 1024);
                    if size_mb > self.config.max_file_size_mb as u64 {
                        let reason = format!(
                            "File '{}' is {}MB, exceeds limit of {}MB",
                            path.display(),
                            size_mb,
                            self.config.max_file_size_mb
                        );
                        self.record_audit("deny", "file_size", &path.display().to_string(), &reason);
                        return Err(reason);
                    }
                }
                Err(_) => {} // File might not exist yet for writes
            }
        }
        Ok(())
    }

    // ── Command checking ──

    /// Check if a terminal command is allowed.
    /// Merges built-in dangerous command detection with user-configured blocked commands.
    pub fn check_command(&self, cmd: &str) -> Result<(), String> {
        let lower = cmd.to_lowercase();

        // Check auto_allow_commands (bypass all checks)
        for allowed in &self.config.permissions.auto_allow_commands {
            if lower.contains(&allowed.to_lowercase()) {
                return Ok(());
            }
        }

        // Check built-in dangerous patterns (always enforced)
        if let Some(reason) = is_dangerous_command(cmd) {
            self.record_audit("deny", "command", cmd, reason);
            return Err(reason.to_string());
        }

        // Skip blocked_commands check for known-safe commands
        if is_safe_command(cmd) {
            return Ok(());
        }

        // Check user-configured blocked commands
        for blocked in &self.config.blocked_commands {
            if lower.contains(&blocked.to_lowercase()) {
                let reason = format!(
                    "Command matches blocked pattern: '{}'",
                    blocked
                );
                self.record_audit("deny", "command", cmd, &reason);
                return Err(reason);
            }
        }

        // Check confirm_commands (require user confirmation)
        for confirm in &self.config.permissions.confirm_commands {
            if lower.contains(&confirm.to_lowercase()) {
                let reason = format!(
                    "Command matches confirm pattern: '{}'",
                    confirm
                );
                self.record_audit("confirm", "command", cmd, &reason);
                return Err(format!("[CONFIRM_REQUIRED] {}", reason));
            }
        }

        Ok(())
    }

    // ── URL checking ──

    /// Check if a URL is allowed (SSRF protection + domain whitelist).
    pub fn check_url(&self, url: &str) -> Result<(), String> {
        // Parse URL to extract host
        let host = extract_host(url);

        // SSRF protection: block private/internal IPs in strict mode
        if self.config.mode == SandboxMode::Strict {
            if let Some(ref h) = host {
                if is_private_host(h) {
                    let reason = format!(
                        "URL '{}' targets a private/internal address (blocked in strict mode)",
                        url
                    );
                    self.record_audit("deny", "url", url, &reason);
                    return Err(reason);
                }
            }
        }

        // Domain whitelist check (empty = all allowed)
        if !self.config.allowed_domains.is_empty() {
            if let Some(ref h) = host {
                let host_lower = h.to_lowercase();
                let allowed = self.config.allowed_domains.iter().any(|d| {
                    let d_lower = d.to_lowercase();
                    host_lower == d_lower || host_lower.ends_with(&format!(".{}", d_lower))
                });
                if !allowed {
                    let reason = format!(
                        "URL domain '{}' is not in allowed domains list",
                        h
                    );
                    self.record_audit("deny", "url", url, &reason);
                    return Err(reason);
                }
            }
        }

        Ok(())
    }

    // ── Audit logging ──

    fn record_audit(&self, action: &str, tool: &str, target: &str, reason: &str) {
        let record = AuditRecord {
            timestamp: chrono::Utc::now().to_rfc3339(),
            action: action.to_string(),
            tool: tool.to_string(),
            target: target.to_string(),
            reason: reason.to_string(),
        };

        // Store in memory
        if let Ok(mut records) = self.audit_records.lock() {
            records.push(record.clone());
            // Keep last 1000 records in memory
            if records.len() > 1000 {
                let drain_count = records.len() - 1000;
                records.drain(..drain_count);
            }
        }

        // Append to file
        if let Some(ref log_path) = self.audit_log_path {
            let line = record.to_log_line();
            if let Some(parent) = log_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
            {
                let _ = writeln!(file, "{}", line);
            }
        }

        log::warn!("[Sandbox] {} {} target=\"{}\" reason=\"{}\"", action, tool, target, reason);
    }

    /// Get recent audit records
    pub fn get_recent_audits(&self, n: usize) -> Vec<AuditRecord> {
        if let Ok(records) = self.audit_records.lock() {
            records.iter().rev().take(n).cloned().collect()
        } else {
            Vec::new()
        }
    }
}

// ── Built-in safe commands that bypass blocked_commands checks ──

/// Commands that are always safe and should never be blocked by user-configured patterns.
const SAFE_COMMAND_PREFIXES: &[&str] = &[
    "ls", "dir", "cat", "type", "head", "tail", "echo", "pwd",
    "find", "grep", "rg", "fd", "wc", "sort", "uniq", "diff",
    "which", "where", "whoami", "hostname", "date", "env",
    "git status", "git log", "git diff", "git branch", "git show",
    "git remote", "git stash list", "git tag", "git reflog",
    "cargo check", "cargo build", "cargo test", "cargo clippy",
    "npm run", "npm test", "npx tsc", "node", "python", "python3",
    "tree", "stat", "file", "du", "df", "ps", "top",
];

/// Check if a command starts with a known safe command prefix.
fn is_safe_command(cmd: &str) -> bool {
    let trimmed = cmd.trim().to_lowercase();
    SAFE_COMMAND_PREFIXES.iter().any(|safe| {
        trimmed.starts_with(safe)
            && (trimmed.len() == safe.len()
                || trimmed.as_bytes().get(safe.len()).map_or(false, |&b| b == b' ' || b == b'\t' || b == b'-' || b == b'.'))
    })
}

// ── Built-in dangerous command detection ──

/// Check if a command matches known dangerous patterns.
/// Returns Some(reason) if dangerous, None if safe.
fn is_dangerous_command(cmd: &str) -> Option<&'static str> {
    let lower = cmd.to_lowercase();
    let trimmed = lower.trim();

    // Recursive deletion of root/home
    if trimmed.contains("rm -rf /") || trimmed.contains("rm -rf ~") || trimmed.contains("rm -rf /*") {
        return Some("rm -rf on root/home directory is forbidden");
    }
    // Disk formatting
    if trimmed.starts_with("mkfs.") || trimmed.starts_with("format ") || trimmed.starts_with("dd if=") {
        return Some("disk formatting/dd operations are forbidden");
    }
    // Fork bomb
    if trimmed.contains(":()") && trimmed.contains(";:") {
        return Some("fork bomb detected");
    }
    // System-level dangerous operations
    if trimmed.contains("chmod 777 /") || trimmed.contains("chmod -r 777 /") {
        return Some("chmod 777 on root is forbidden");
    }
    // Git force push to main branches
    if (trimmed.contains("git push") || trimmed.contains("git-push"))
        && (trimmed.contains("--force") || trimmed.contains(" -f "))
        && (trimmed.contains(" main") || trimmed.contains(" master") || trimmed.contains("main ") || trimmed.contains("master "))
    {
        return Some("force push to main/master is forbidden");
    }
    // Deleting important system directories
    for sys_dir in &[
        "/etc", "/boot", "/sys", "/proc", "/dev", "/usr", "/bin", "/sbin", "/var",
        "c:\\windows", "c:\\program files", "c:\\program files (x86)",
    ] {
        if trimmed.contains(&format!("rm -rf {}", sys_dir))
            || trimmed.contains(&format!("rmdir {}", sys_dir))
            || trimmed.contains(&format!("rm -rf \"{}", sys_dir))
        {
            return Some("deleting system directory is forbidden");
        }
    }
    // Shutdown/reboot
    if trimmed.starts_with("shutdown ") || trimmed.starts_with("reboot ")
        || trimmed == "shutdown" || trimmed == "reboot"
        || trimmed.starts_with("init 0") || trimmed.starts_with("init 6")
    {
        return Some("shutdown/reboot commands are forbidden");
    }
    // Pipe to shell from network
    if (trimmed.contains("curl ") || trimmed.contains("wget "))
        && (trimmed.contains(" | sh") || trimmed.contains(" | bash") || trimmed.contains(" | zsh"))
    {
        return Some("curl/wget pipe to shell is forbidden (potential remote code execution)");
    }
    // PowerShell dangerous patterns
    if trimmed.contains("remove-item") && (trimmed.contains("c:\\windows") || trimmed.contains("c:\\program")) {
        return Some("removing system directories via PowerShell is forbidden");
    }
    if trimmed.contains("set-executionpolicy") && trimmed.contains("unrestricted") {
        return Some("disabling PowerShell execution policy is forbidden");
    }
    // Registry manipulation
    if trimmed.contains("reg delete") || trimmed.contains("reg add") {
        if trimmed.contains("hklm") || trimmed.contains("hkey_local_machine") {
            return Some("modifying system registry is forbidden");
        }
    }

    None
}

// ── URL helpers ──

/// Extract the host from a URL string
fn extract_host(url: &str) -> Option<String> {
    // Try to parse as URL
    let stripped = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .or_else(|| url.strip_prefix("ftp://"))
        .unwrap_or(url);

    // Take the part before the first / or :
    let host_port = stripped.split('/').next().unwrap_or(stripped);
    let host = host_port.split(':').next().unwrap_or(host_port);

    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Check if a hostname resolves to a private/internal IP
fn is_private_host(host: &str) -> bool {
    let lower = host.to_lowercase();

    // Localhost variants
    if lower == "localhost" || lower == "127.0.0.1" || lower == "::1" || lower == "0.0.0.0" {
        return true;
    }

    // Private IP ranges
    if let Ok(addr) = lower.parse::<std::net::Ipv4Addr>() {
        let octets = addr.octets();
        // 10.0.0.0/8
        if octets[0] == 10 {
            return true;
        }
        // 172.16.0.0/12
        if octets[0] == 172 && (16..=31).contains(&octets[1]) {
            return true;
        }
        // 192.168.0.0/16
        if octets[0] == 192 && octets[1] == 168 {
            return true;
        }
        // 169.254.0.0/16 (link-local)
        if octets[0] == 169 && octets[1] == 254 {
            return true;
        }
    }

    // Common internal hostnames
    if lower.ends_with(".local") || lower.ends_with(".internal") || lower.ends_with(".corp") {
        return true;
    }

    false
}
