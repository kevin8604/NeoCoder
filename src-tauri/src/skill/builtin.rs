/// Built-in default Skills (embedded as constants).
/// These are used when no user-defined skill files exist on disk.
pub const REVIEW_SKILL: &str = r#"---
name: review
description: Review code for quality, correctness, and best practices
trigger: /review
mode: agent
agent: reviewer
---

You are performing a code review. Analyze the following code thoroughly.

Focus on:
- Bugs and logic errors
- Security vulnerabilities
- Performance issues
- Code style and readability
- Best practices for the language

## Code under review
```$LANGUAGE
$SELECTION
```

## File context
`$FILE_PATH`

$ARGUMENTS

Provide specific, actionable feedback with line references where possible. Rate the overall code quality (1-5) and summarize key findings.
"#;

pub const EXPLAIN_SKILL: &str = r#"---
name: explain
description: Explain what the selected code does
trigger: /explain
mode: ask
---

Explain the following code in detail. Cover:
1. What the code does (high-level purpose)
2. How it works (step-by-step logic)
3. Key patterns or techniques used
4. Any potential issues or edge cases

## Code
```$LANGUAGE
$SELECTION
```

## File context
`$FILE_PATH`

$ARGUMENTS

Provide a clear, structured explanation suitable for a developer who may be unfamiliar with this code.
"#;

pub const REFACTOR_SKILL: &str = r#"---
name: refactor
description: Suggest refactoring improvements for the selected code
trigger: /refactor
mode: edit
---

Analyze the following code and suggest refactoring improvements.

Consider:
- Reducing complexity and improving readability
- Extracting reusable components
- Applying design patterns
- Improving naming and structure
- Removing code duplication

## Current code
```$LANGUAGE
$SELECTION
```

## File context
`$FILE_PATH`

$ARGUMENTS

Provide the refactored code with explanations for each change. Use the edit code block format:
```language:path/to/file
// refactored code
```
"#;

pub const TESTS_SKILL: &str = r#"---
name: tests
description: Generate unit tests for the selected code
trigger: /tests
mode: agent
agent: code_writer
---

Generate comprehensive unit tests for the following code.

Requirements:
- Cover all public functions and methods
- Include edge cases and error scenarios
- Use idiomatic testing patterns for the language
- Aim for high coverage of branches and conditions

## Code to test
```$LANGUAGE
$SELECTION
```

## File context
`$FILE_PATH`

$ARGUMENTS

Write the test file and explain the test strategy. If the code is in a file, create a corresponding test file (e.g., `foo.rs` -> `foo_test.rs` or `test_foo.rs`).
"#;

pub const AUTO_REVIEW_SKILL: &str = r#"---
name: auto-review
description: Automatically review code changes (git diff) for issues
trigger: /auto-review
mode: agent
agent: reviewer
---

You are performing an automated code review on recent changes.

Analyze the following git diff and provide a structured review.

## Review Criteria
- **Bugs**: Logic errors, null pointer risks, race conditions
- **Security**: Injection vulnerabilities, data exposure, auth issues
- **Performance**: N+1 queries, unnecessary allocations, blocking calls
- **Style**: Naming conventions, code organization, readability
- **Best Practices**: Error handling, edge cases, documentation

## Changes to Review
```diff
$DIFF
```

$ARGUMENTS

## Output Format
Provide your review in this format:

### Summary
Brief overview of the changes and overall assessment.

### Issues Found
List any issues with severity (🔴 Critical / 🟡 Warning / 🔵 Suggestion):
- [severity] file:line - description

### Recommendations
Actionable suggestions for improvement.

### Verdict
✅ Approve / ⚠️ Approve with comments / ❌ Request changes
"#;

/// Multi-agent workflow template: orchestrates reviewer → code_writer → debugger
/// → reviewer pipeline for structured multi-stage tasks.
pub const WORKFLOW_SKILL: &str = r#"---
name: workflow
description: Run a multi-agent workflow (plan → code → test → review)
trigger: /workflow
mode: agent
agent: orchestrator
---

You are orchestrating a multi-agent workflow for the following task.

## Workflow stages (run in order, each with the dedicated sub-agent):
1. **Analysis** — use sub_agent(reviewer) to analyze the task and identify affected areas
2. **Implementation** — use sub_agent(code_writer) to implement the changes
3. **Verification** — use run_tests (or run_test) to verify; if tests fail, use sub_agent(debugger) to fix
4. **Review** — use sub_agent(reviewer) to review the final diff

## Task
$ARGUMENTS

## Rules
- Do NOT skip stages: every stage must complete before the next begins
- Between stages, summarize the outcome in a short line (what changed / what was verified)
- If a stage fails (tests red, review critical), iterate on that stage up to 2 extra times before escalating
- Final response must contain: what was implemented, test results, and the review verdict
"#;

/// Returns all built-in skill file contents as (filename, content) pairs.
pub fn builtin_skills() -> Vec<(&'static str, &'static str)> {
    vec![
        ("review.md", REVIEW_SKILL),
        ("explain.md", EXPLAIN_SKILL),
        ("refactor.md", REFACTOR_SKILL),
        ("tests.md", TESTS_SKILL),
        ("auto-review.md", AUTO_REVIEW_SKILL),
        ("workflow.md", WORKFLOW_SKILL),
    ]
}
