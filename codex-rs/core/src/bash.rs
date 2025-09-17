use tree_sitter::Node;
use tree_sitter::Parser;
use tree_sitter::Tree;
use tree_sitter_bash::LANGUAGE as BASH;

/// Parse the provided bash source using tree-sitter-bash, returning a Tree on
/// success or None if parsing failed.
pub fn try_parse_bash(bash_lc_arg: &str) -> Option<Tree> {
    let lang = BASH.into();
    let mut parser = Parser::new();
    #[expect(clippy::expect_used)]
    parser.set_language(&lang).expect("load bash grammar");
    let old_tree: Option<&Tree> = None;
    parser.parse(bash_lc_arg, old_tree)
}

/// Parse a script which may contain multiple simple commands joined only by
/// the safe logical/pipe/sequencing operators: `&&`, `||`, `;`, `|`.
///
/// Returns `Some(Vec<command_words>)` if every command is a plain word‑only
/// command and the parse tree does not contain disallowed constructs
/// (parentheses, redirections, substitutions, control flow, etc.). Otherwise
/// returns `None`.
pub fn try_parse_word_only_commands_sequence(tree: &Tree, src: &str) -> Option<Vec<Vec<String>>> {
    if tree.root_node().has_error() {
        return None;
    }

    // List of allowed (named) node kinds for a "word only commands sequence".
    // If we encounter a named node that is not in this list we reject.
    const ALLOWED_KINDS: &[&str] = &[
        // top level containers
        "program",
        "list",
        "pipeline",
        // commands & words
        "command",
        "command_name",
        "word",
        "string",
        "string_content",
        "raw_string",
        "number",
    ];
    // Allow only safe punctuation / operator tokens; anything else causes reject.
    const ALLOWED_PUNCT_TOKENS: &[&str] = &["&&", "||", ";", "|", "\"", "'"];

    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut stack = vec![root];
    let mut command_nodes = Vec::new();
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        if node.is_named() {
            if !ALLOWED_KINDS.contains(&kind) {
                return None;
            }
            if kind == "command" {
                command_nodes.push(node);
            }
        } else {
            // Reject any punctuation / operator tokens that are not explicitly allowed.
            if !(ALLOWED_PUNCT_TOKENS.contains(&kind) || kind.trim().is_empty()) {
                // If it's a quote token or operator it's allowed above; we also allow whitespace tokens.
                // Any other punctuation like parentheses, braces, redirects, backticks, etc are rejected.
                return None;
            }
        }
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }

    let mut commands = Vec::new();
    for node in command_nodes {
        if let Some(words) = extract_words_from_command_node(node, src) {
            commands.push(words);
        } else {
            return None;
        }
    }
    Some(commands)
}

/// Extract the plain words of a simple command node, normalizing quoted
/// strings into their contents. Returns None if the node contains unsupported
/// constructs for a word-only command.
pub(crate) fn extract_words_from_command_node(cmd: Node, src: &str) -> Option<Vec<String>> {
    if cmd.kind() != "command" {
        return None;
    }
    let mut words = Vec::new();
    let mut cursor = cmd.walk();
    for child in cmd.named_children(&mut cursor) {
        match child.kind() {
            "command_name" => {
                let word_node = child.named_child(0)?;
                if word_node.kind() != "word" {
                    return None;
                }
                words.push(word_node.utf8_text(src.as_bytes()).ok()?.to_owned());
            }
            "word" | "number" => {
                words.push(child.utf8_text(src.as_bytes()).ok()?.to_owned());
            }
            "string" => {
                if child.child_count() == 3
                    && child.child(0)?.kind() == "\""
                    && child.child(1)?.kind() == "string_content"
                    && child.child(2)?.kind() == "\""
                {
                    words.push(child.child(1)?.utf8_text(src.as_bytes()).ok()?.to_owned());
                } else {
                    return None;
                }
            }
            "raw_string" => {
                let raw_string = child.utf8_text(src.as_bytes()).ok()?;
                let stripped = raw_string
                    .strip_prefix('\'')
                    .and_then(|s| s.strip_suffix('\''));
                if let Some(s) = stripped {
                    words.push(s.to_owned());
                } else {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(words)
}

/// Find the earliest `command` node in source order within the parse tree.
pub(crate) fn find_first_command_node(tree: &Tree) -> Option<Node<'_>> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut stack = vec![root];
    let mut best: Option<Node> = None;
    while let Some(node) = stack.pop() {
        if node.is_named()
            && node.kind() == "command"
            && best.is_none_or(|b| node.start_byte() < b.start_byte())
        {
            best = Some(node);
        }
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    best
}

/// Given the first command node, return the byte index in `src` at which the
/// remainder script starts, only if the next non-whitespace token is an allowed
/// sequencing operator for dropping the leading `cd`.
///
/// Allowed operators: `&&` (conditional on success) and `;` (unconditional).
/// Disallowed: `||`, `|` — removing `cd` would change semantics.
pub(crate) fn remainder_start_after_wrapper_operator(first_cmd: Node, src: &str) -> Option<usize> {
    let mut sib = first_cmd.next_sibling()?;
    while !sib.is_named() && sib.kind().trim().is_empty() {
        sib = sib.next_sibling()?;
    }
    if sib.is_named() || (sib.kind() != "&&" && sib.kind() != ";") {
        return None;
    }
    let mut idx = sib.end_byte();
    let bytes = src.as_bytes();
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    if idx >= bytes.len() { None } else { Some(idx) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_seq(src: &str) -> Option<Vec<Vec<String>>> {
        let tree = try_parse_bash(src)?;
        try_parse_word_only_commands_sequence(&tree, src)
    }

    #[test]
    fn accepts_single_simple_command() {
        let cmds = parse_seq("ls -1").unwrap();
        assert_eq!(cmds, vec![vec!["ls".to_string(), "-1".to_string()]]);
    }

    #[test]
    fn accepts_multiple_commands_with_allowed_operators() {
        let src = "ls && pwd; echo 'hi there' | wc -l";
        let cmds = parse_seq(src).unwrap();
        let expected: Vec<Vec<String>> = vec![
            vec!["wc".to_string(), "-l".to_string()],
            vec!["echo".to_string(), "hi there".to_string()],
            vec!["pwd".to_string()],
            vec!["ls".to_string()],
        ];
        assert_eq!(cmds, expected);
    }

    #[test]
    fn extracts_double_and_single_quoted_strings() {
        let cmds = parse_seq("echo \"hello world\"").unwrap();
        assert_eq!(
            cmds,
            vec![vec!["echo".to_string(), "hello world".to_string()]]
        );

        let cmds2 = parse_seq("echo 'hi there'").unwrap();
        assert_eq!(
            cmds2,
            vec![vec!["echo".to_string(), "hi there".to_string()]]
        );
    }

    #[test]
    fn accepts_numbers_as_words() {
        let cmds = parse_seq("echo 123 456").unwrap();
        assert_eq!(
            cmds,
            vec![vec![
                "echo".to_string(),
                "123".to_string(),
                "456".to_string()
            ]]
        );
    }

    #[test]
    fn rejects_parentheses_and_subshells() {
        assert!(parse_seq("(ls)").is_none());
        assert!(parse_seq("ls || (pwd && echo hi)").is_none());
    }

    #[test]
    fn rejects_redirections_and_unsupported_operators() {
        assert!(parse_seq("ls > out.txt").is_none());
        assert!(parse_seq("echo hi & echo bye").is_none());
    }

    #[test]
    fn rejects_command_and_process_substitutions_and_expansions() {
        assert!(parse_seq("echo $(pwd)").is_none());
        assert!(parse_seq("echo `pwd`").is_none());
        assert!(parse_seq("echo $HOME").is_none());
        assert!(parse_seq("echo \"hi $USER\"").is_none());
    }

    #[test]
    fn rejects_variable_assignment_prefix() {
        assert!(parse_seq("FOO=bar ls").is_none());
    }

    #[test]
    fn rejects_trailing_operator_parse_error() {
        assert!(parse_seq("ls &&").is_none());
    }

    #[test]
    fn find_first_command_node_finds_cd() {
        let src = "cd foo && ls; git status";
        let tree = try_parse_bash(src).unwrap();
        let first = find_first_command_node(&tree).unwrap();
        let words = extract_words_from_command_node(first, src).unwrap();
        assert_eq!(words, vec!["cd".to_string(), "foo".to_string()]);
    }

    #[test]
    fn remainder_after_wrapper_operator_allows_and_and_semicolon() {
        // Allows &&
        let src = "cd foo && ls; git status";
        let tree = try_parse_bash(src).unwrap();
        let first = find_first_command_node(&tree).unwrap();
        let idx = remainder_start_after_wrapper_operator(first, src).unwrap();
        assert_eq!(&src[idx..], "ls; git status");

        // Allows ;
        let src2 = "cd foo; ls";
        let tree2 = try_parse_bash(src2).unwrap();
        let first2 = find_first_command_node(&tree2).unwrap();
        let idx2 = remainder_start_after_wrapper_operator(first2, src2).unwrap();
        assert_eq!(&src2[idx2..], "ls");
    }

    #[test]
    fn remainder_after_wrapper_operator_rejects_or_and_pipe() {
        // Rejects ||
        let src = "cd foo || echo hi";
        let tree = try_parse_bash(src).unwrap();
        let first = find_first_command_node(&tree).unwrap();
        assert!(remainder_start_after_wrapper_operator(first, src).is_none());

        // Rejects |
        let src2 = "cd foo | rg bar";
        let tree2 = try_parse_bash(src2).unwrap();
        let first2 = find_first_command_node(&tree2).unwrap();
        assert!(remainder_start_after_wrapper_operator(first2, src2).is_none());
    }
}
