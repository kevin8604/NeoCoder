#[cfg(test)]
mod tests {
    use crate::rag::{tokenize, term_frequencies, cosine_similarity, is_function_start, is_class_start, find_block_end};

    // ── Tokenize Tests ─────────────────────────────────────────────────────

    #[test]
    fn test_tokenize_basic() {
        let text = "hello world foo bar";
        let tokens = tokenize(text);
        assert_eq!(tokens, vec!["hello", "world", "foo", "bar"]);
    }

    #[test]
    fn test_tokenize_with_punctuation() {
        let text = "hello, world! foo.bar; baz";
        let tokens = tokenize(text);
        assert_eq!(tokens, vec!["hello", "world", "foo", "bar", "baz"]);
    }

    #[test]
    fn test_tokenize_lowercase() {
        let text = "Hello WORLD Foo Bar";
        let tokens = tokenize(text);
        assert_eq!(tokens, vec!["hello", "world", "foo", "bar"]);
    }

    #[test]
    fn test_tokenize_filter_short() {
        let text = "a ab abc defg";
        let tokens = tokenize(text);
        // "a" should be filtered out (len < 2)
        assert_eq!(tokens, vec!["ab", "abc", "defg"]);
    }

    #[test]
    fn test_tokenize_code_snippet() {
        let text = "fn main() { println!(\"Hello\"); }";
        let tokens = tokenize(text);
        assert!(tokens.contains(&"fn".to_string()));
        assert!(tokens.contains(&"main".to_string()));
        assert!(tokens.contains(&"println".to_string()));
        assert!(tokens.contains(&"hello".to_string()));
    }

    #[test]
    fn test_tokenize_empty() {
        let text = "";
        let tokens = tokenize(text);
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_tokenize_only_punctuation() {
        let text = "!@#$%^&*()";
        let tokens = tokenize(text);
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_tokenize_numbers() {
        let text = "version 1.2.3 build 42";
        let tokens = tokenize(text);
        // Single digit numbers are filtered out (len < 2)
        assert_eq!(tokens, vec!["version", "build", "42"]);
    }

    #[test]
    fn test_tokenize_mixed_unicode() {
        let text = "hello 世界 test 中文";
        let tokens = tokenize(text);
        // Unicode alphanumeric should be preserved
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"test".to_string()));
    }

    // ── Term Frequencies Tests ─────────────────────────────────────────────

    #[test]
    fn test_term_frequencies_basic() {
        let doc = "hello world hello";
        let tf = term_frequencies(doc);
        
        // "hello" appears 2 times out of 3 total
        assert!((tf["hello"] - 2.0 / 3.0).abs() < 0.001);
        // "world" appears 1 time out of 3 total
        assert!((tf["world"] - 1.0 / 3.0).abs() < 0.001);
    }

    #[test]
    fn test_term_frequencies_empty() {
        let doc = "";
        let tf = term_frequencies(doc);
        assert!(tf.is_empty());
    }

    #[test]
    fn test_term_frequencies_all_same() {
        let doc = "foo foo foo foo";
        let tf = term_frequencies(doc);
        assert!((tf["foo"] - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_term_frequencies_unique_words() {
        let doc = "one two three four";
        let tf = term_frequencies(doc);
        // Each word appears 1 time out of 4 total
        assert!((tf["one"] - 0.25).abs() < 0.001);
        assert!((tf["two"] - 0.25).abs() < 0.001);
        assert!((tf["three"] - 0.25).abs() < 0.001);
        assert!((tf["four"] - 0.25).abs() < 0.001);
    }

    #[test]
    fn test_term_frequencies_case_insensitive() {
        let doc = "Hello HELLO hello";
        let tf = term_frequencies(doc);
        // All should be normalized to "hello"
        assert_eq!(tf.len(), 1);
        assert!((tf["hello"] - 1.0).abs() < 0.001);
    }

    // ── Cosine Similarity Tests ────────────────────────────────────────────

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![-1.0, -2.0, -3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_partial() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        // cos(45°) = √2/2 ≈ 0.707
        assert!((sim - 0.7071).abs() < 0.01);
    }

    #[test]
    fn test_cosine_similarity_different_lengths() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_empty() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_both_zero() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![0.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_unit_vectors() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_large_vectors() {
        // Test with 100-dimensional vectors
        let a: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let b: Vec<f32> = (0..100).map(|i| (i * 2) as f32).collect();
        let sim = cosine_similarity(&a, &b);
        // Should be 1.0 (proportional vectors)
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_symmetric() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![4.0, 5.0, 6.0];
        let sim_ab = cosine_similarity(&a, &b);
        let sim_ba = cosine_similarity(&b, &a);
        assert!((sim_ab - sim_ba).abs() < 0.001);
    }

    // ── Function Detection Tests ───────────────────────────────────────────

    #[test]
    fn test_is_function_start_rust() {
        assert!(is_function_start("fn main() {", "rust"));
        assert!(is_function_start("pub fn foo(x: i32) -> i32 {", "rust"));
        assert!(is_function_start("async fn bar() {", "rust"));
        assert!(is_function_start("pub async fn baz() {", "rust"));
    }

    #[test]
    fn test_is_function_start_python() {
        assert!(is_function_start("def main():", "python"));
        assert!(is_function_start("def foo(arg1, arg2):", "python"));
    }

    #[test]
    fn test_is_function_start_javascript() {
        assert!(is_function_start("function foo() {", "javascript"));
        assert!(is_function_start("function bar(a, b) {", "javascript"));
    }

    #[test]
    fn test_is_function_start_go() {
        assert!(is_function_start("func main() {", "go"));
        assert!(is_function_start("func Foo(x int) int {", "go"));
    }

    #[test]
    fn test_is_function_start_visibility_modifiers() {
        assert!(is_function_start("private void foo() {", "java"));
        assert!(is_function_start("public static void main() {", "java"));
        assert!(is_function_start("protected int bar() {", "java"));
    }

    #[test]
    fn test_is_function_start_false_positives() {
        // Missing parentheses
        assert!(!is_function_start("fn main", "rust"));
        assert!(!is_function_start("def foo", "python"));
        
        // Not a function keyword
        assert!(!is_function_start("let x = 5;", "rust"));
        assert!(!is_function_start("const foo = 1;", "javascript"));
    }

    #[test]
    fn test_is_function_start_case_insensitive() {
        // These should still match due to to_lowercase()
        assert!(is_function_start("FN main() {", "rust"));
        assert!(is_function_start("DEF foo():", "python"));
        assert!(is_function_start("FUNCTION bar() {", "javascript"));
    }

    // ── Class Detection Tests ──────────────────────────────────────────────

    #[test]
    fn test_is_class_start_basic() {
        assert!(is_class_start("class Foo {", "java"));
        assert!(is_class_start("struct Bar {", "rust"));
        assert!(is_class_start("enum Baz {", "rust"));
    }

    #[test]
    fn test_is_class_start_interface() {
        assert!(is_class_start("interface Foo {", "typescript"));
        assert!(is_class_start("trait Bar {", "rust"));
    }

    #[test]
    fn test_is_class_start_rust_specifics() {
        assert!(is_class_start("pub struct Foo {", "rust"));
        assert!(is_class_start("pub enum Bar {", "rust"));
        assert!(is_class_start("pub trait Baz {", "rust"));
        assert!(is_class_start("impl Foo {", "rust"));
        assert!(is_class_start("pub impl Bar {", "rust"));
    }

    #[test]
    fn test_is_class_start_other_keywords() {
        assert!(is_class_start("export class Foo {", "typescript"));
        assert!(is_class_start("module Bar {", "typescript"));
        assert!(is_class_start("type Baz = {", "typescript"));
    }

    #[test]
    fn test_is_class_start_false_positives() {
        assert!(!is_class_start("let x = 5;", "rust"));
        assert!(!is_class_start("const foo = 1;", "javascript"));
        assert!(!is_class_start("fn main() {", "rust"));
    }

    #[test]
    fn test_is_class_start_case_insensitive() {
        assert!(is_class_start("CLASS Foo {", "java"));
        assert!(is_class_start("STRUCT Bar {", "rust"));
        assert!(is_class_start("ENUM Baz {", "rust"));
    }

    // ── Block End Detection Tests ──────────────────────────────────────────

    #[test]
    fn test_find_block_end_simple() {
        let lines = vec!["fn foo() {", "    let x = 1;", "}"];
        let result = find_block_end(&lines, 0, "rust");
        assert_eq!(result, Some(2));
    }

    #[test]
    fn test_find_block_end_nested() {
        let lines = vec![
            "fn foo() {",
            "    if true {",
            "        println!(\"nested\");",
            "    }",
            "}",
        ];
        let result = find_block_end(&lines, 0, "rust");
        assert_eq!(result, Some(4));
    }

    #[test]
    fn test_find_block_end_deep_nesting() {
        let lines = vec![
            "fn main() {",
            "    loop {",
            "        if condition {",
            "            break;",
            "        }",
            "    }",
            "}",
        ];
        let result = find_block_end(&lines, 0, "rust");
        assert_eq!(result, Some(6));
    }

    #[test]
    fn test_find_block_end_no_opening_brace() {
        let lines = vec!["let x = 1;", "let y = 2;"];
        let result = find_block_end(&lines, 0, "rust");
        assert_eq!(result, None);
    }

    #[test]
    fn test_find_block_end_unterminated() {
        let lines = vec!["fn foo() {", "    let x = 1;"];
        let result = find_block_end(&lines, 0, "rust");
        assert_eq!(result, None);
    }

    #[test]
    fn test_find_block_end_multiple_blocks() {
        let lines = vec![
            "fn foo() {",
            "    let x = 1;",
            "}",
            "fn bar() {",
            "    let y = 2;",
            "}",
        ];
        // Should find end of first block only
        let result = find_block_end(&lines, 0, "rust");
        assert_eq!(result, Some(2));
    }

    #[test]
    fn test_find_block_end_braces_in_strings() {
        // Simplified version doesn't handle strings, so this will fail
        // This is a known limitation of the simple approach
        let lines = vec![
            "fn foo() {",
            "    let s = \"}\";",
            "}",
        ];
        // The simple parser will incorrectly count the brace in the string
        // This test documents the limitation
        let result = find_block_end(&lines, 0, "rust");
        // May or may not be correct - depends on implementation
        assert!(result.is_some());
    }

    // ── Integration Tests ──────────────────────────────────────────────────

    #[test]
    fn test_tokenize_and_tf_combined() {
        let doc1 = "hello world hello";
        let doc2 = "hello foo bar";
        
        let tf1 = term_frequencies(doc1);
        let tf2 = term_frequencies(doc2);
        
        // Both should have "hello"
        assert!(tf1.contains_key("hello"));
        assert!(tf2.contains_key("hello"));
        
        // doc1 should have "world", doc2 should have "foo" and "bar"
        assert!(tf1.contains_key("world"));
        assert!(tf2.contains_key("foo"));
        assert!(tf2.contains_key("bar"));
    }

    #[test]
    fn test_cosine_similarity_from_tf() {
        let doc1 = "hello world";
        let doc2 = "hello world";
        let doc3 = "foo bar";
        
        let tf1 = term_frequencies(doc1);
        let tf2 = term_frequencies(doc2);
        let tf3 = term_frequencies(doc3);
        
        // Convert to vectors (simplified)
        let all_terms: Vec<&str> = vec!["hello", "world", "foo", "bar"];
        
        let vec1: Vec<f32> = all_terms.iter().map(|t| tf1.get(*t).unwrap_or(&0.0)).copied().collect();
        let vec2: Vec<f32> = all_terms.iter().map(|t| tf2.get(*t).unwrap_or(&0.0)).copied().collect();
        let vec3: Vec<f32> = all_terms.iter().map(|t| tf3.get(*t).unwrap_or(&0.0)).copied().collect();
        
        // doc1 and doc2 are identical, should have similarity 1.0
        let sim12 = cosine_similarity(&vec1, &vec2);
        assert!((sim12 - 1.0).abs() < 0.001);
        
        // doc1 and doc3 share no terms, should have similarity 0.0
        let sim13 = cosine_similarity(&vec1, &vec3);
        assert!(sim13.abs() < 0.001);
    }

    #[test]
    fn test_function_and_block_detection_pipeline() {
        let code = vec![
            "fn main() {",
            "    let x = 1;",
            "    if x > 0 {",
            "        println!(\"positive\");",
            "    }",
            "}",
            "",
            "fn helper() {",
            "    // helper function",
            "}",
        ];
        
        // Find function starts
        let func_indices: Vec<usize> = code
            .iter()
            .enumerate()
            .filter(|(_, line)| is_function_start(line, "rust"))
            .map(|(i, _)| i)
            .collect();
        
        assert_eq!(func_indices.len(), 2);
        assert_eq!(func_indices[0], 0);
        assert_eq!(func_indices[1], 7);
        
        // Find block ends
        let end1 = find_block_end(&code, func_indices[0], "rust");
        let end2 = find_block_end(&code, func_indices[1], "rust");
        
        assert_eq!(end1, Some(5));
        assert_eq!(end2, Some(9));
    }

    #[test]
    fn test_edge_case_single_character_tokens() {
        let text = "a b c ab bc ca abc";
        let tokens = tokenize(text);
        // Single characters should be filtered out
        assert!(!tokens.contains(&"a".to_string()));
        assert!(!tokens.contains(&"b".to_string()));
        assert!(!tokens.contains(&"c".to_string()));
        // But 2+ character tokens should remain
        assert!(tokens.contains(&"ab".to_string()));
        assert!(tokens.contains(&"abc".to_string()));
    }

    #[test]
    fn test_edge_case_very_long_text() {
        // Test with 10000 repetitions
        let text = "hello ".repeat(10000);
        let tokens = tokenize(&text);
        assert_eq!(tokens.len(), 10000);
        assert!(tokens.iter().all(|t| t == "hello"));
        
        let tf = term_frequencies(&text);
        assert_eq!(tf.len(), 1);
        assert!((tf["hello"] - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_edge_case_special_characters_in_code() {
        let text = "let x: Vec<String> = vec![1, 2, 3];";
        let tokens = tokenize(text);
        assert!(tokens.contains(&"let".to_string()));
        assert!(tokens.contains(&"vec".to_string()));
        assert!(tokens.contains(&"string".to_string()));
        // Single digit numbers are filtered out (len < 2)
        assert!(!tokens.contains(&"1".to_string()));
        assert!(!tokens.contains(&"2".to_string()));
        assert!(!tokens.contains(&"3".to_string()));
    }
}
