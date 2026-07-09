// Phase 6: Code extraction from Edit tool calls
// Uses tree-sitter to parse files and extract the relevant code region

use tree_sitter::Parser;

fn get_ts_language(language_id: &str) -> Option<tree_sitter::Language> {
    match language_id {
        "js" | "javascript" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" | "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "py" | "python" => Some(tree_sitter_python::LANGUAGE.into()),
        _ => None,
    }
}

pub fn check_syntax(code: &str, language_id: &str) -> Option<String> {
    let ts_lang = get_ts_language(language_id)?;
    let mut parser = Parser::new();
    if parser.set_language(&ts_lang).is_err() {
        return None;
    }

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
    let ts_lang = get_ts_language(language_id)?;
    let mut parser = Parser::new();
    if parser.set_language(&ts_lang).is_err() {
        return None;
    }

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
