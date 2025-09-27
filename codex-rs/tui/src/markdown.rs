use codex_core::config::Config;
use codex_core::config_types::UriBasedFileOpener;
use ratatui::text::Line;
use std::path::Path;

pub(crate) fn append_markdown(
    markdown_source: &str,
    lines: &mut Vec<Line<'static>>,
    config: &Config,
) {
    append_markdown_with_opener_and_cwd(
        markdown_source,
        lines,
        config.file_opener,
        &config.cwd,
        Some(&config.syntax_highlight_theme),
    );
}

fn append_markdown_with_opener_and_cwd(
    markdown_source: &str,
    lines: &mut Vec<Line<'static>>,
    file_opener: UriBasedFileOpener,
    cwd: &Path,
    syntax_highlight_theme: Option<&str>,
) {
    let rendered = crate::markdown_render::render_markdown_text_with_citations(
        markdown_source,
        file_opener.get_scheme(),
        cwd,
        syntax_highlight_theme,
    );
    crate::render::line_utils::push_owned_lines(&rendered.lines, lines);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn citations_not_rewritten_inside_code_blocks() {
        let src = "Before 【F:/x.rs†L1】\n```\nInside 【F:/x.rs†L2】\n```\nAfter 【F:/x.rs†L3】\n";
        let cwd = Path::new("/");
        let mut out = Vec::new();
        append_markdown_with_opener_and_cwd(src, &mut out, UriBasedFileOpener::VsCode, cwd, None);
        let rendered: Vec<String> = out
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.clone())
                    .collect::<String>()
            })
            .collect();
        // Expect a line containing the inside text unchanged.
        assert!(rendered.iter().any(|s| s.contains("Inside 【F:/x.rs†L2】")));
        // And first/last sections rewritten.
        assert!(
            rendered
                .first()
                .map(|s| s.contains("vscode://file"))
                .unwrap_or(false)
        );
        assert!(
            rendered
                .last()
                .map(|s| s.contains("vscode://file"))
                .unwrap_or(false)
        );
    }

    #[test]
    fn indented_code_blocks_preserve_leading_whitespace() {
        // Basic sanity: indented code with surrounding blank lines should produce the indented line.
        let src = "Before\n\n    code 1\n\nAfter\n";
        let cwd = Path::new("/");
        let mut out = Vec::new();
        append_markdown_with_opener_and_cwd(src, &mut out, UriBasedFileOpener::None, cwd, None);
        let lines: Vec<String> = out
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.clone())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(
            lines,
            vec![
                "Before".to_string(),
                "".to_string(),
                "    code 1".to_string(),
                "".to_string(),
                "After".to_string()
            ]
        );
    }

    #[test]
    fn citations_not_rewritten_inside_indented_code_blocks() {
        let src = "Start 【F:/x.rs†L1】\n\n    Inside 【F:/x.rs†L2】\n\nEnd 【F:/x.rs†L3】\n";
        let cwd = Path::new("/");
        let mut out = Vec::new();
        append_markdown_with_opener_and_cwd(src, &mut out, UriBasedFileOpener::VsCode, cwd, None);
        let rendered: Vec<String> = out
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.clone())
                    .collect::<String>()
            })
            .collect();
        assert!(
            rendered
                .iter()
                .any(|s| s.contains("Start") && s.contains("vscode://file"))
        );
        assert!(
            rendered
                .iter()
                .any(|s| s.contains("End") && s.contains("vscode://file"))
        );
        assert!(rendered.iter().any(|s| s.contains("Inside 【F:/x.rs†L2】")));
    }

    #[test]
    fn append_markdown_preserves_full_text_line() {
        let src = "Hi! How can I help with codex-rs today? Want me to explore the repo, run tests, or work on a specific change?\n";
        let cwd = Path::new("/");
        let mut out = Vec::new();
        append_markdown_with_opener_and_cwd(src, &mut out, UriBasedFileOpener::None, cwd, None);
        assert_eq!(
            out.len(),
            1,
            "expected a single rendered line for plain text"
        );
        let rendered: String = out
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.clone())
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(
            rendered,
            "Hi! How can I help with codex-rs today? Want me to explore the repo, run tests, or work on a specific change?"
        );
    }

    #[test]
    fn append_markdown_keeps_ordered_list_line_unsplit_in_context() {
        let src = "Loose vs. tight list items:\n1. Tight item\n";
        let cwd = Path::new("/");
        let mut out = Vec::new();
        append_markdown_with_opener_and_cwd(src, &mut out, UriBasedFileOpener::None, cwd, None);

        let lines: Vec<String> = out
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.clone())
                    .collect::<String>()
            })
            .collect();

        // Expect to find the ordered list line rendered as a single line,
        // not split into a marker-only line followed by the text.
        assert!(
            lines.iter().any(|s| s == "1. Tight item"),
            "expected '1. Tight item' rendered as a single line; got: {lines:?}"
        );
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_syntax_highlighting() {
        let cwd = Path::new("/");

        let markdown = r#"
```rust
fn main() {
    println!("Hello, world!");
}
```
"#;

        // Process the markdown with the public API
        let mut lines = Vec::new();
        append_markdown_with_opener_and_cwd(
            markdown,
            &mut lines,
            UriBasedFileOpener::None,
            cwd,
            None,
        );

        // Verify we have some lines of output
        assert!(!lines.is_empty(), "Should have generated some output lines");

        // Verify at least one non-reset RGB foreground (real highlighting)
        let has_colored_fg = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| matches!(span.style.fg, Some(ratatui::style::Color::Rgb(_, _, _))))
        });
        assert!(
            has_colored_fg,
            "Expected at least one non-reset foreground color from syntax highlighting"
        );
    }
}
