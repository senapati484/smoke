// Phase 6: Code extraction from Edit tool calls
// Uses tree-sitter to parse files and extract the relevant code region
//
// Also exposes a tiny "roll-your-own" pattern detector (detect_roll_your_own)
// that returns a soft hint when the written code re-implements something the
// stdlib or a well-known library already provides. The intent is to surface
// the question to the agent ("is this reimplementation necessary?") without
// blocking the write.

use std::collections::HashMap;
use std::sync::Mutex;
use tree_sitter::Parser;

fn get_ts_language(language_id: &str) -> Option<tree_sitter::Language> {
    match language_id {
        "js" | "javascript" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" | "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "py" | "python" => Some(tree_sitter_python::LANGUAGE.into()),
        "rs" | "rust" => Some(tree_sitter_rust::LANGUAGE.into()),
        _ => None,
    }
}

// ── Parser cache (S6) ────────────────────────────────────────────────────────
//
// tree-sitter's `set_language` is called once per `Parser::new()`. That's
// ~1-5ms of pure work per call, which adds up when an agent writes a dozen
// small files in a row. We memoize: one Parser per language, lazily
// initialized, shared across all syntax checks and snippet extractions.
//
// `tree_sitter::Parser` is `!Send` (it owns C state not safe to share
// across threads), so we wrap it in a Mutex. The lock is only held during
// `parse()` — which is the only operation we do on the parser — and the
// hook runs in a single async task, so contention is essentially zero.

use std::sync::OnceLock;

static PARSER_CACHE: OnceLock<Mutex<HashMap<&'static str, Parser>>> = OnceLock::new();

fn parser_cache() -> &'static Mutex<HashMap<&'static str, Parser>> {
    PARSER_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get a parser for the given language, creating it on first use.
/// The returned MutexGuard holds the parser; release it after `parse()`.
fn get_or_create_parser<'a>(
    cache: &'a Mutex<HashMap<&'static str, Parser>>,
    language_id: &str,
) -> Option<std::sync::MutexGuard<'a, HashMap<&'static str, Parser>>> {
    let lang_key: &'static str = match language_id {
        "js" | "javascript" => "js",
        "ts" | "typescript" => "ts",
        "tsx" => "tsx",
        "py" | "python" => "py",
        "rs" | "rust" => "rs",
        _ => return None,
    };
    let mut cache = cache.lock().ok()?;
    if !cache.contains_key(lang_key) {
        let ts_lang = get_ts_language(lang_key)?;
        let mut parser = Parser::new();
        if parser.set_language(&ts_lang).is_err() {
            return None;
        }
        cache.insert(lang_key, parser);
    }
    Some(cache)
}

pub fn check_syntax(code: &str, language_id: &str) -> Option<String> {
    let mut guard = get_or_create_parser(parser_cache(), language_id)?;
    let parser = guard.get_mut(language_id_to_key(language_id))?;

    let tree = parser.parse(code, None)?;
    let root = tree.root_node();
    if root.has_error() {
        if let Some(err_node) = find_error_node(root) {
            let start = err_node.start_position();
            return Some(format!(
                "Syntax error at line {}, column {}",
                start.row + 1,
                start.column + 1
            ));
        }
        return Some("Syntax error in file".to_string());
    }
    None
}

pub fn extract_enclosing_function(code: &str, edit_start: usize, language_id: &str) -> Option<String> {
    let mut guard = get_or_create_parser(parser_cache(), language_id)?;
    let parser = guard.get_mut(language_id_to_key(language_id))?;

    let tree = parser.parse(code, None)?;
    let root = tree.root_node();

    // Walk up from the descendant covering the edit start byte
    let mut current_node = root.descendant_for_byte_range(edit_start, edit_start)?;
    loop {
        let kind = current_node.kind();
        if kind == "function_definition"
            || kind == "class_definition"
            || kind == "function_declaration"
            || kind == "method_definition"
            || kind == "class_declaration"
            || kind == "class_expression"
            || kind == "arrow_function"
            || kind == "function_item"
            || kind == "struct_item"
            || kind == "enum_item"
            || kind == "impl_item"
            || kind == "trait_item"
        {
            let start = current_node.start_byte();
            let end = current_node.end_byte();
            if start < end && end <= code.len() {
                return Some(code[start..end].to_string());
            }
        }
        if let Some(parent) = current_node.parent() {
            current_node = parent;
        } else {
            break;
        }
    }
    None
}

fn language_id_to_key(id: &str) -> &'static str {
    match id {
        "js" | "javascript" => "js",
        "ts" | "typescript" => "ts",
        "tsx" => "tsx",
        "py" | "python" => "py",
        "rs" | "rust" => "rs",
        _ => "",
    }
}

fn find_error_node(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    if node.is_error() || node.is_missing() {
        return Some(node);
    }
    for i in 0..(node.child_count() as u32) {
        if let Some(child) = node.child(i) {
            if child.has_error() {
                if let Some(err) = find_error_node(child) {
                    return Some(err);
                }
            }
        }
    }
    None
}

// ── Roll-your-own pattern detector ────────────────────────────────────────────

/// Patterns we recognize as candidates for stdlib/library replacement.
///
/// Each pattern is matched with simple `contains` checks — no regex, no AST
/// traversal. This is intentionally narrow to keep false positives near zero.
/// Adding a new pattern: append a `(name, matcher, hint)` triple. The matcher
/// should be distinctive enough that it almost never appears in code that
/// legitimately needs the custom implementation.
type PatternMatcher = fn(&str) -> bool;

const ROLL_YOUR_OWN_PATTERNS: &[(&str, PatternMatcher, &str)] = &[
    (
        "debounce",
        |c| contains_word(c, "debounce"),
        "Detected a custom `debounce` implementation. Lodash provides `debounce(fn, wait)`; the custom version may be unnecessary unless the project has a deliberate no-dependency policy.",
    ),
    (
        "throttle",
        |c| contains_word(c, "throttle"),
        "Detected a custom `throttle` implementation. Lodash provides `throttle(fn, wait)`; the custom version may be unnecessary.",
    ),
    (
        "deep-clone-via-json",
        |c| c.contains("JSON.parse(JSON.stringify("),
        "Detected a deep-clone implemented as `JSON.parse(JSON.stringify(...))`. This loses functions, Dates, Maps, Sets, and circular references. The built-in `structuredClone()` (Node 17+, all modern browsers) handles all of these correctly.",
    ),
    (
        "deep-clone-named",
        |c| contains_word(c, "deepClone") || contains_word(c, "deep_clone") || contains_word(c, "deepclone"),
        "Detected a custom `deepClone` function. The built-in `structuredClone()` (Node 17+, all modern browsers) is the standard replacement unless the project needs to clone class instances or handle non-cloneable types.",
    ),
    (
        "uuid-by-hand",
        |c| c.contains("Math.random().toString(36") || c.contains(".toString(36).substring"),
        "Detected a hand-rolled UUID/random-string generator using `Math.random().toString(36)`. This is not cryptographically secure and has collision risk. The built-in `crypto.randomUUID()` (Node 14.17+, all modern browsers) is collision-free and secure.",
    ),
    (
        "manual-chunking",
        |c| {
            contains_word(c, "chunk") && (c.contains("function chunk") || c.contains("const chunk =") || c.contains("def chunk"))
        },
        "Detected a custom `chunk` function (splitting an array into fixed-size batches). Python's `more-itertools.chunked`, JavaScript's `lodash.chunk`, or a generator expression is the standard replacement.",
    ),
    (
        "manual-retry",
        |c| {
            // Look for a retry loop: a function named retry/withRetry/retryN that contains a loop + sleep/delay/wait
            let has_retry_fn = contains_word(c, "retry") || contains_word(c, "withRetry") || contains_word(c, "retryRequest");
            let has_loop = c.contains("for (") || c.contains("while (") || c.contains("for i in");
            let has_sleep = c.contains("sleep") || c.contains("setTimeout") || c.contains("await new Promise") || c.contains("time.sleep");
            has_retry_fn && has_loop && has_sleep
        },
        "Detected a manual retry-with-backoff implementation. The `tenacity` library (Python), `async-retry` (Node.js), or `tokio-retry` (Rust) handle retries, backoff, and jitter correctly with much less code.",
    ),
    (
        "manual-memoize",
        |c| {
            let has_memoize_fn = contains_word(c, "memoize") || contains_word(c, "memoisation") || contains_word(c, "memo");
            let has_cache_map = c.contains("new Map") || c.contains("= {}") || c.contains("cache[");
            has_memoize_fn && has_cache_map
        },
        "Detected a custom memoization helper. Use `lodash.memoize` (JS/TS), Python's `functools.lru_cache` or `functools.cache` decorator, or Rust's `once_cell::sync::Lazy` for one-time initialization.",
    ),
    (
        "manual-flatten",
        |c| {
            // Custom flatten: function named flatten that uses concat or reduce
            (contains_word(c, "flatten") || contains_word(c, "flatMap"))
                && (c.contains(".reduce(") || c.contains(".concat(") || c.contains("[].concat"))
                && (c.contains("function flatten") || c.contains("const flatten =") || c.contains("def flatten"))
        },
        "Detected a custom `flatten` function. Use `Array.prototype.flat()` (all modern environments) or `Array.prototype.flatMap()` for a one-step map+flatten.",
    ),
    (
        "manual-event-emitter",
        |c| {
            // Custom event system with on/emit/off pattern
            let has_on = c.contains(".on(") || c.contains("addEventListener");
            let has_emit = contains_word(c, "emit") || contains_word(c, "dispatch");
            let has_listeners_map = c.contains("listeners") || c.contains("handlers") || c.contains("subscribers");
            has_on && has_emit && has_listeners_map
                && (c.contains("function EventEmitter") || c.contains("class EventEmitter")
                    || c.contains("class EventBus") || c.contains("const emitter"))
        },
        "Detected a custom EventEmitter/EventBus implementation. Node.js ships `require('events').EventEmitter`; for the browser use `EventTarget` (native) or `mitt` (600-byte zero-dep package).",
    ),
    (
        "manual-sleep",
        |c| {
            // sleep helper: const sleep = ms => new Promise(...)
            (contains_word(c, "sleep") || contains_word(c, "delay") || contains_word(c, "wait"))
                && c.contains("new Promise")
                && (c.contains("setTimeout") || c.contains("resolve"))
                && (c.contains("const sleep") || c.contains("function sleep")
                    || c.contains("const delay") || c.contains("function delay")
                    || c.contains("const wait") || c.contains("function wait"))
        },
        "Detected a hand-rolled `sleep`/`delay` helper (`new Promise(r => setTimeout(r, ms))`). This is fine as a one-liner inline, but if it appears in a utility file the package `delay` (npm) is a zero-config drop-in.",
    ),
    (
        "manual-is-empty",
        |c| {
            // Custom isEmpty checking Object.keys length or array length
            (contains_word(c, "isEmpty") || contains_word(c, "is_empty"))
                && (c.contains("Object.keys(") || c.contains(".length === 0") || c.contains(".length == 0"))
                && (c.contains("function isEmpty") || c.contains("const isEmpty ="))
        },
        "Detected a custom `isEmpty` utility. Lodash provides `_.isEmpty(value)` which handles arrays, objects, strings, Maps, and Sets uniformly. Alternatively, `Object.keys(obj).length === 0` inline is idiomatic for plain objects.",
    ),
    (
        "python-retry",
        |c| {
            // Python: manual try/except with loop and time.sleep — typical retry pattern
            let has_loop = c.contains("for _ in range") || c.contains("while True") || c.contains("for attempt");
            let has_sleep = c.contains("time.sleep");
            let has_except = c.contains("except ") && c.contains("except:");
            has_loop && has_sleep && (c.contains("except ") || has_except)
        },
        "Detected a Python manual retry loop with `time.sleep`. The `tenacity` library (`pip install tenacity`) provides battle-tested retry logic with decorators, exponential back-off, and jitter in a few lines.",
    ),
];

/// Returns a list of stdlib-replacement hints for the given code, or an empty
/// Vec if nothing matches. The order of hints matches the order of patterns
/// in `ROLL_YOUR_OWN_PATTERNS`.
pub fn detect_roll_your_own(code: &str, language_id: &str) -> Vec<&'static str> {
    // Language filter: skip patterns that don't apply.
    let applicable: &[&str] = match language_id {
        "py" | "python" => &[
            "manual-chunking",
            "manual-memoize",
            "manual-retry",
            "python-retry",
        ],
        "js" | "javascript" | "ts" | "typescript" | "tsx" | "jsx" => &[
            "debounce",
            "throttle",
            "deep-clone-via-json",
            "deep-clone-named",
            "uuid-by-hand",
            "manual-chunking",
            "manual-retry",
            "manual-memoize",
            "manual-flatten",
            "manual-event-emitter",
            "manual-sleep",
            "manual-is-empty",
        ],
        _ => &[],
    };

    let mut hints = Vec::new();
    for (name, matcher, hint) in ROLL_YOUR_OWN_PATTERNS {
        if applicable.contains(name) && matcher(code) {
            hints.push(*hint);
        }
    }
    hints
}

/// Whether the writing-side stdlib-hint prompt should fire.
///
/// Combines the size gate (don't pester on tiny edits) with the pattern
/// detector. Returns the joined hint string if the prompt should fire, or
/// `None` otherwise.
///
/// `size_threshold` is the minimum number of new lines that must be added
/// before the prompt fires. The hook reads this from `cfg.prompts.writing_size_threshold`.
/// A value of `0` disables the size gate (always eligible when a pattern matches).
pub fn writing_hint_for(
    code: &str,
    language_id: &str,
    added_lines: usize,
    size_threshold: usize,
) -> Option<String> {
    if size_threshold > 0 && added_lines < size_threshold {
        return None;
    }
    let hints = detect_roll_your_own(code, language_id);
    if hints.is_empty() {
        return None;
    }
    Some(format!(
        "Detected {} in the newly-written code: {}\nConsider whether the standard library or a well-known package already provides this, or whether the custom implementation is necessary.",
        if hints.len() == 1 { "a pattern" } else { "patterns" },
        hints.join(" / ")
    ))
}

/// Whole-word substring check: `c` contains `word` as a token, not as a
/// substring of another identifier. For example, `contains_word("debounced", "debounce")`
/// is false. This is a tiny inline implementation to avoid pulling in `regex`.
fn contains_word(c: &str, word: &str) -> bool {
    c.match_indices(word).any(|(i, _)| {
        let before_ok = i == 0 || !is_ident_char(c.as_bytes()[i - 1]);
        let after = i + word.len();
        let after_ok = after >= c.len() || !is_ident_char(c.as_bytes()[after]);
        before_ok && after_ok
    })
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_roll_your_own_finds_debounce() {
        let code = "const debounce = (fn, wait) => {\n  let t;\n  return (...args) => {\n    clearTimeout(t);\n    t = setTimeout(fn, wait, ...args);\n  };\n};\n";
        let hints = detect_roll_your_own(code, "js");
        assert!(!hints.is_empty(), "expected debounce hint, got nothing");
        assert!(hints[0].contains("debounce"), "hint should mention debounce: {}", hints[0]);
    }

    #[test]
    fn detect_roll_your_own_finds_deep_clone_via_json() {
        let code = "function deepClone(obj) { return JSON.parse(JSON.stringify(obj)); }";
        let hints = detect_roll_your_own(code, "ts");
        assert!(!hints.is_empty(), "expected deep-clone hint");
        assert!(hints[0].contains("structuredClone"), "hint should mention structuredClone: {}", hints[0]);
    }

    #[test]
    fn detect_roll_your_own_finds_uuid_by_hand() {
        let code = "function uuid() { return Math.random().toString(36).substring(2); }";
        let hints = detect_roll_your_own(code, "js");
        assert!(!hints.is_empty(), "expected uuid-by-hand hint");
        assert!(hints[0].contains("randomUUID"), "hint should mention randomUUID: {}", hints[0]);
    }

    #[test]
    fn detect_roll_your_own_silent_on_clean_code() {
        let code = "function add(a, b) { return a + b; }\nconsole.log(add(1, 2));\n";
        let hints = detect_roll_your_own(code, "js");
        assert!(hints.is_empty(), "expected no hints on plain code, got: {:?}", hints);
    }

    #[test]
    fn detect_roll_your_own_does_not_match_substring() {
        // "debounced" should NOT match "debounce" pattern
        let code = "// this is a debounced version of the function\n";
        let hints = detect_roll_your_own(code, "js");
        assert!(hints.is_empty(), "substring 'debounced' should not match 'debounce' pattern, got: {:?}", hints);
    }

    #[test]
    fn writing_hint_respects_size_threshold() {
        let code = "function debounce(fn) { /* ... */ }";
        // Only 1 line added — should not fire (default 100-line threshold)
        assert!(writing_hint_for(code, "js", 1, 100).is_none());
        // 100 lines added — exactly at threshold, should fire
        let hint = writing_hint_for(code, "js", 100, 100);
        assert!(hint.is_some(), "should fire at threshold (100 lines)");
    }

    #[test]
    fn writing_hint_respects_custom_threshold() {
        let code = "function debounce(fn) { /* ... */ }";
        // With threshold=200 and only 50 lines added, should NOT fire
        assert!(writing_hint_for(code, "js", 50, 200).is_none());
        // With threshold=200 and 250 lines added, should fire
        assert!(writing_hint_for(code, "js", 250, 200).is_some());
        // With threshold=0, size gate is disabled — even 1 line should fire
        assert!(writing_hint_for(code, "js", 1, 0).is_some());
    }

    #[test]
    fn writing_hint_silent_when_no_patterns() {
        let code = "function add(a, b) { return a + b; }";
        assert!(writing_hint_for(code, "js", 200, 100).is_none());
    }

    #[test]
    fn contains_word_basic() {
        assert!(contains_word("const debounce = 1;", "debounce"));
        assert!(contains_word("function debounce() {}", "debounce"));
        assert!(!contains_word("debounced = 1;", "debounce"));
        assert!(!contains_word("myDebounce = 1;", "debounce"));
        assert!(contains_word("// debounce is great", "debounce"));
    }

    /// Parser cache smoke test (S6): calling check_syntax twice for the same
    /// language should reuse the cached parser. We can't directly observe
    /// the cache from outside, but we can verify that repeated calls return
    /// consistent results (which they wouldn't if a new parser was somehow
    /// configured differently each time).
    #[test]
    fn parser_cache_repeated_calls_are_consistent() {
        let code = "let x = 1;\nfunction add(a, b) { return a + b; }\n";
        // First call: populates the cache
        let r1 = check_syntax(code, "js");
        let r2 = check_syntax(code, "js");
        let r3 = check_syntax(code, "js");
        assert_eq!(r1, r2);
        assert_eq!(r2, r3);
        assert!(r1.is_none(), "valid code should not produce a syntax error");

        // Calling with a different language should not interfere
        let r_py = check_syntax("x = 1", "py");
        assert!(r_py.is_none());
        // JS still works
        let r_js_again = check_syntax(code, "js");
        assert_eq!(r_js_again, r1);
    }

    /// extract_enclosing_function should also benefit from the parser cache.
    #[test]
    fn extract_enclosing_function_with_cache() {
        let code = "function greet(name) { return `Hello ${name}`; }\n";
        let idx = code.find("return").unwrap();
        let s1 = extract_enclosing_function(code, idx, "js");
        let s2 = extract_enclosing_function(code, idx, "js");
        assert!(s1.is_some());
        assert_eq!(s1, s2, "cached parser should produce identical results");
    }
}
