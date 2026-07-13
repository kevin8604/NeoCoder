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

/// Returns all built-in skill file contents as (filename, content) pairs.
pub fn builtin_skills() -> Vec<(&'static str, &'static str)> {
    vec![
        ("review.md", REVIEW_SKILL),
        ("explain.md", EXPLAIN_SKILL),
        ("refactor.md", REFACTOR_SKILL),
        ("tests.md", TESTS_SKILL),
    ]
}
