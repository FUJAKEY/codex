use crate::citation_regex::CITATION_REGEX;
use pulldown_cmark::CodeBlockKind;
use pulldown_cmark::CowStr;
use pulldown_cmark::Event;
use pulldown_cmark::HeadingLevel;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;
#[cfg(feature = "syntax-highlighting")]
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use std::borrow::Cow;
use std::path::Path;

#[cfg(feature = "syntax-highlighting")]
use once_cell::sync::Lazy;
#[cfg(feature = "syntax-highlighting")]
use syntect::easy::HighlightLines;
#[cfg(feature = "syntax-highlighting")]
use syntect::highlighting::Color as SyntectColor;
#[cfg(feature = "syntax-highlighting")]
use syntect::highlighting::ThemeSet;
#[cfg(feature = "syntax-highlighting")]
use syntect::parsing::SyntaxSet;

#[allow(clippy::disallowed_methods)]
#[cfg(feature = "syntax-highlighting")]
const DEFAULT_CODE_BG: Color = Color::Rgb(40, 44, 52);

#[cfg(feature = "syntax-highlighting")]
static SYNTAX_SET: Lazy<SyntaxSet> = Lazy::new(SyntaxSet::load_defaults_newlines);
#[cfg(feature = "syntax-highlighting")]
static THEME: Lazy<ThemeSet> = Lazy::new(ThemeSet::load_defaults);

#[derive(Clone, Debug)]
struct IndentContext {
    prefix: Vec<Span<'static>>,
    marker: Option<Vec<Span<'static>>>,
    is_list: bool,
}

impl IndentContext {
    fn new(prefix: Vec<Span<'static>>, marker: Option<Vec<Span<'static>>>, is_list: bool) -> Self {
        Self {
            prefix,
            marker,
            is_list,
        }
    }
}

#[cfg(feature = "syntax-highlighting")]
fn get_syntax_definition(language: Option<&str>) -> &'static syntect::parsing::SyntaxReference {
    let raw = language.unwrap_or("txt").trim().to_lowercase();
    let token = raw
        .split_whitespace()
        .next()
        .unwrap_or("txt")
        .trim_start_matches("{.")
        .trim_start_matches('.')
        .trim_end_matches('}')
        .split(|c| [',', ';'].contains(&c))
        .next()
        .unwrap_or("txt");

    if token.is_empty()
        || matches!(
            token,
            "nohighlight" | "text" | "plain" | "plaintext" | "txt"
        )
    {
        SYNTAX_SET.find_syntax_plain_text()
    } else {
        SYNTAX_SET
            .find_syntax_by_token(token)
            .or_else(|| SYNTAX_SET.find_syntax_by_extension(token))
            .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text())
    }
}

#[cfg(feature = "syntax-highlighting")]
fn syntect_to_ratatui_color(color: SyntectColor) -> Option<Color> {
    if color.a == 0 {
        None
    } else {
        Some(Color::Rgb(color.r, color.g, color.b))
    }
}

#[allow(dead_code)]
pub(crate) fn render_markdown_text(input: &str) -> Text<'static> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(input, options);
    let mut w = Writer::new(parser, None, None, None);
    w.run();
    w.text
}

#[allow(dead_code)]
pub(crate) fn render_markdown_text_with_citations(
    input: &str,
    scheme: Option<&str>,
    cwd: &Path,
    syntax_highlight_theme: Option<&str>,
) -> Text<'static> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(input, options);
    let mut w = Writer::new(
        parser,
        scheme.map(str::to_string),
        Some(cwd.to_path_buf()),
        syntax_highlight_theme.map(str::to_string),
    );
    w.run();
    w.text
}

struct Writer<'a, I>
where
    I: Iterator<Item = Event<'a>>,
{
    iter: I,
    text: Text<'static>,
    inline_styles: Vec<Style>,
    indent_stack: Vec<IndentContext>,
    list_indices: Vec<Option<u64>>,
    link: Option<String>,
    needs_newline: bool,
    pending_marker_line: bool,
    in_paragraph: bool,
    scheme: Option<String>,
    cwd: Option<std::path::PathBuf>,
    in_code_block: bool,
    #[cfg(feature = "syntax-highlighting")]
    syntax_highlight_theme: Option<String>,
    #[cfg(feature = "syntax-highlighting")]
    current_highlighter: Option<HighlightLines<'static>>,
    #[cfg(feature = "syntax-highlighting")]
    current_code_lang: Option<String>,
}

impl<'a, I> Writer<'a, I>
where
    I: Iterator<Item = Event<'a>>,
{
    fn new(
        iter: I,
        scheme: Option<String>,
        cwd: Option<std::path::PathBuf>,
        _syntax_highlight_theme: Option<String>,
    ) -> Self {
        Self {
            iter,
            text: Text::default(),
            inline_styles: Vec::new(),
            indent_stack: Vec::new(),
            list_indices: Vec::new(),
            link: None,
            needs_newline: false,
            pending_marker_line: false,
            in_paragraph: false,
            scheme,
            cwd,
            in_code_block: false,
            #[cfg(feature = "syntax-highlighting")]
            syntax_highlight_theme: _syntax_highlight_theme,
            #[cfg(feature = "syntax-highlighting")]
            current_highlighter: None,
            #[cfg(feature = "syntax-highlighting")]
            current_code_lang: None,
        }
    }

    fn run(&mut self) {
        while let Some(ev) = self.iter.next() {
            self.handle_event(ev);
        }
    }

    fn handle_event(&mut self, event: Event<'a>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.text(text),
            Event::Code(code) => self.code(code),
            Event::SoftBreak => self.soft_break(),
            Event::HardBreak => self.hard_break(),
            Event::Rule => {
                if !self.text.lines.is_empty() {
                    self.push_blank_line();
                }
                self.push_line(Line::from("———"));
                self.needs_newline = true;
            }
            Event::Html(html) => self.html(html, false),
            Event::InlineHtml(html) => self.html(html, true),
            Event::FootnoteReference(_) => {}
            Event::TaskListMarker(_) => {}
        }
    }

    fn start_tag(&mut self, tag: Tag<'a>) {
        match tag {
            Tag::Paragraph => self.start_paragraph(),
            Tag::Heading { level, .. } => self.start_heading(level),
            Tag::BlockQuote => self.start_blockquote(),
            Tag::CodeBlock(kind) => {
                let indent = match kind {
                    CodeBlockKind::Fenced(_) => None,
                    CodeBlockKind::Indented => Some(Span::from(" ".repeat(4))),
                };
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) => Some(lang.to_string()),
                    CodeBlockKind::Indented => None,
                };
                self.start_codeblock(lang, indent)
            }
            Tag::List(start) => self.start_list(start),
            Tag::Item => self.start_item(),
            Tag::Emphasis => self.push_inline_style(Style::new().italic()),
            Tag::Strong => self.push_inline_style(Style::new().bold()),
            Tag::Strikethrough => self.push_inline_style(Style::new().crossed_out()),
            Tag::Link { dest_url, .. } => self.push_link(dest_url.to_string()),
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::Image { .. }
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.end_paragraph(),
            TagEnd::Heading(_) => self.end_heading(),
            TagEnd::BlockQuote => self.end_blockquote(),
            TagEnd::CodeBlock => self.end_codeblock(),
            TagEnd::List(_) => self.end_list(),
            TagEnd::Item => {
                self.indent_stack.pop();
                self.pending_marker_line = false;
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_inline_style(),
            TagEnd::Link => self.pop_link(),
            TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::TableCell
            | TagEnd::Image
            | TagEnd::MetadataBlock(_) => {}
        }
    }

    fn start_paragraph(&mut self) {
        if self.needs_newline {
            self.push_blank_line();
        }
        self.push_line(Line::default());
        self.needs_newline = false;
        self.in_paragraph = true;
    }

    fn end_paragraph(&mut self) {
        self.needs_newline = true;
        self.in_paragraph = false;
        self.pending_marker_line = false;
    }

    fn start_heading(&mut self, level: HeadingLevel) {
        if self.needs_newline {
            self.push_line(Line::default());
            self.needs_newline = false;
        }
        let heading_style = match level {
            HeadingLevel::H1 => Style::new().bold().underlined(),
            HeadingLevel::H2 => Style::new().bold(),
            HeadingLevel::H3 => Style::new().bold().italic(),
            HeadingLevel::H4 => Style::new().italic(),
            HeadingLevel::H5 => Style::new().italic(),
            HeadingLevel::H6 => Style::new().italic(),
        };
        let content = format!("{} ", "#".repeat(level as usize));
        self.push_line(Line::from(vec![Span::styled(content, heading_style)]));
        self.push_inline_style(heading_style);
        self.needs_newline = false;
    }

    fn end_heading(&mut self) {
        self.needs_newline = true;
        self.pop_inline_style();
    }

    fn start_blockquote(&mut self) {
        if self.needs_newline {
            self.push_blank_line();
            self.needs_newline = false;
        }
        self.indent_stack
            .push(IndentContext::new(vec![Span::from("> ")], None, false));
    }

    fn end_blockquote(&mut self) {
        self.indent_stack.pop();
        self.needs_newline = true;
    }

    fn text(&mut self, text: CowStr<'a>) {
        if self.pending_marker_line {
            self.push_line(Line::default());
        }
        self.pending_marker_line = false;

        // Don't add extra newlines for code blocks or list items
        if !self.in_code_block
            && self.needs_newline
            && !self.indent_stack.iter().any(|ctx| ctx.is_list)
        {
            self.push_line(Line::default());
            self.needs_newline = false;
        }

        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            // Only add newlines between lines if not in a code block and not a list item continuation
            if i > 0 {
                let is_list_continuation = self.indent_stack.iter().any(|ctx| ctx.is_list)
                    && !line.trim_start().is_empty()
                    && !line.starts_with('-')
                    && !line.starts_with('*')
                    && !line.starts_with('+')
                    && !line.starts_with(|c: char| c.is_ascii_digit());

                if !self.in_code_block && !is_list_continuation {
                    self.push_line(Line::default());
                }
            }
            if self.in_code_block {
                #[allow(unused_mut)]
                let mut highlighted = false;
                #[cfg(feature = "syntax-highlighting")]
                if let Some(highlighter) = &mut self.current_highlighter {
                    let ranges: Vec<(syntect::highlighting::Style, &str)> = highlighter
                        .highlight_line(line, &SYNTAX_SET)
                        .unwrap_or_else(|_| vec![(syntect::highlighting::Style::default(), line)]);
                    let spans: Vec<Span> = ranges
                        .into_iter()
                        .map(|(syn_style, text)| {
                            let fg = syn_style.foreground;
                            let mut style = Style::default()
                                .fg(syntect_to_ratatui_color(fg).unwrap_or(Color::Reset));
                            if let Some(bg_color) = syntect_to_ratatui_color(syn_style.background) {
                                style = style.bg(bg_color);
                            } else {
                                style = style.bg(DEFAULT_CODE_BG);
                            }
                            Span::styled(text.to_string(), style)
                        })
                        .collect();
                    self.push_line(Line::from(spans));
                    highlighted = true;
                }
                if !highlighted {
                    // No syntax highlighting, just push the line as is
                    self.push_line(Line::from(line.to_string()));
                }
                if !highlighted {
                    // No syntax highlighting, just push the line as is
                    self.push_line(Line::from(line.to_string()));
                }
            } else {
                let mut content = line.to_string();
                if let (Some(scheme), Some(cwd)) = (&self.scheme, &self.cwd) {
                    let cow =
                        rewrite_file_citations_with_scheme(&content, Some(scheme.as_str()), cwd);
                    if let std::borrow::Cow::Owned(s) = cow {
                        content = s;
                    }
                }
                let span = Span::styled(
                    content,
                    self.inline_styles.last().copied().unwrap_or_default(),
                );
                self.push_span(span);
            }
        }
        self.needs_newline = false;
    }

    fn code(&mut self, code: CowStr<'a>) {
        if self.pending_marker_line {
            self.push_line(Line::default());
            self.pending_marker_line = false;
        }
        let span = Span::from(code.into_string()).dim();
        self.push_span(span);
    }

    fn html(&mut self, html: CowStr<'a>, inline: bool) {
        self.pending_marker_line = false;
        for (i, line) in html.lines().enumerate() {
            if self.needs_newline {
                self.push_line(Line::default());
                self.needs_newline = false;
            }
            if i > 0 {
                self.push_line(Line::default());
            }
            let style = self.inline_styles.last().copied().unwrap_or_default();
            self.push_span(Span::styled(line.to_string(), style));
        }
        self.needs_newline = !inline;
    }

    fn hard_break(&mut self) {
        self.push_line(Line::default());
    }

    fn soft_break(&mut self) {
        self.push_line(Line::default());
    }

    fn start_list(&mut self, index: Option<u64>) {
        if self.list_indices.is_empty() && self.needs_newline {
            self.push_line(Line::default());
        }
        self.list_indices.push(index);
    }

    fn end_list(&mut self) {
        self.list_indices.pop();
        self.needs_newline = true;
    }

    fn start_item(&mut self) {
        self.pending_marker_line = true;
        let depth = self.list_indices.len();
        let is_ordered = self
            .list_indices
            .last()
            .map(Option::is_some)
            .unwrap_or(false);
        let width = depth * 4 - 3;
        let marker = if let Some(last_index) = self.list_indices.last_mut() {
            match last_index {
                None => {
                    // Bullet point list item
                    let marker = " ".repeat(width - 1) + "- ";
                    Some(vec![Span::from(marker)])
                }
                Some(index) => {
                    // Ordered list item
                    *index += 1;
                    let marker = format!("{:width$}. ", *index - 1);
                    // Apply light_blue to first three levels of ordered lists
                    if depth <= 3 {
                        Some(vec![marker.light_blue()])
                    } else {
                        Some(vec![marker.into()])
                    }
                }
            }
        } else {
            None
        };

        let indent_prefix = if depth == 0 {
            Vec::new()
        } else {
            let indent_len = if is_ordered { width + 2 } else { width + 1 };
            vec![Span::from(" ".repeat(indent_len))]
        };

        // Check if we're continuing a list item
        if let Some(last_line) = self.text.lines.last_mut()
            && !last_line.spans.is_empty()
            && let Some(last_span) = last_line.spans.last()
            && last_span.content.ends_with(' ')
        {
            // Remove the trailing space to avoid double spaces
            if let Some(last_span) = last_line.spans.last_mut() {
                last_span.content = last_span.content.trim_end().to_string().into();
            }
        }

        self.indent_stack
            .push(IndentContext::new(indent_prefix, marker, true));
        self.needs_newline = false;
    }

    fn start_codeblock(&mut self, _lang: Option<String>, indent: Option<Span<'static>>) {
        if !self.text.lines.is_empty() {
            self.push_blank_line();
        }
        self.in_code_block = true;
        #[cfg(feature = "syntax-highlighting")]
        {
            self.current_code_lang = _lang.clone();
            if let Some(lang) = &_lang {
                let syntax = get_syntax_definition(Some(lang));
                let theme = match self
                    .syntax_highlight_theme
                    .as_ref()
                    .and_then(|t| THEME.themes.get(t))
                    .or_else(|| THEME.themes.get("base16-ocean.dark"))
                    .or_else(|| THEME.themes.get("InspiredGitHub"))
                    .or_else(|| THEME.themes.values().next())
                {
                    Some(t) => t,
                    None => {
                        self.current_highlighter = None;
                        return;
                    }
                };
                self.current_highlighter = Some(HighlightLines::new(syntax, theme));
            } else {
                self.current_highlighter = None;
            }
        }
        self.indent_stack.push(IndentContext::new(
            vec![indent.unwrap_or_default()],
            None,
            false,
        ));
        // let opener = match lang {
        //     Some(l) if !l.is_empty() => format!("```{l}"),
        //     _ => "```".to_string(),
        // };
        // self.push_line(opener.into());
        self.needs_newline = true;
    }

    fn end_codeblock(&mut self) {
        // self.push_line("```".into());
        self.needs_newline = true;
        self.in_code_block = false;
        #[cfg(feature = "syntax-highlighting")]
        {
            self.current_highlighter = None;
            self.current_code_lang = None;
        }
        self.indent_stack.pop();
    }

    fn push_inline_style(&mut self, style: Style) {
        let current = self.inline_styles.last().copied().unwrap_or_default();
        let merged = current.patch(style);
        self.inline_styles.push(merged);
    }

    fn pop_inline_style(&mut self) {
        self.inline_styles.pop();
    }

    fn push_link(&mut self, dest_url: String) {
        self.link = Some(dest_url);
    }

    fn pop_link(&mut self) {
        if let Some(link) = self.link.take() {
            self.push_span(" (".into());
            self.push_span(link.cyan().underlined());
            self.push_span(")".into());
        }
    }

    fn push_line(&mut self, line: Line<'static>) {
        let mut line = line;
        let was_pending = self.pending_marker_line;
        let mut spans = self.current_prefix_spans();
        spans.append(&mut line.spans);
        let blockquote_active = self
            .indent_stack
            .iter()
            .any(|ctx| ctx.prefix.iter().any(|s| s.content.contains('>')));
        let style = if blockquote_active {
            Style::new().green()
        } else {
            line.style
        };
        self.text.lines.push(Line::from_iter(spans).style(style));
        if was_pending {
            self.pending_marker_line = false;
        }
    }

    fn push_span(&mut self, span: Span<'static>) {
        if let Some(last) = self.text.lines.last_mut() {
            last.push_span(span);
        } else {
            self.push_line(Line::from(vec![span]));
        }
    }

    fn push_blank_line(&mut self) {
        if self.indent_stack.iter().all(|ctx| ctx.is_list) {
            self.text.lines.push(Line::default());
        } else {
            self.push_line(Line::default());
        }
    }

    fn current_prefix_spans(&self) -> Vec<Span<'static>> {
        let mut prefix: Vec<Span<'static>> = Vec::new();
        let last_marker_index = if self.pending_marker_line {
            self.indent_stack
                .iter()
                .enumerate()
                .rev()
                .find_map(|(i, ctx)| if ctx.marker.is_some() { Some(i) } else { None })
        } else {
            None
        };
        let last_list_index = self.indent_stack.iter().rposition(|ctx| ctx.is_list);

        for (i, ctx) in self.indent_stack.iter().enumerate() {
            if self.pending_marker_line {
                if Some(i) == last_marker_index
                    && let Some(marker) = &ctx.marker
                {
                    prefix.extend(marker.iter().cloned());
                    continue;
                }
                if ctx.is_list && last_marker_index.is_some_and(|idx| idx > i) {
                    continue;
                }
            } else if ctx.is_list && Some(i) != last_list_index {
                continue;
            }
            prefix.extend(ctx.prefix.iter().cloned());
        }

        prefix
    }
}

pub(crate) fn rewrite_file_citations_with_scheme<'a>(
    src: &'a str,
    scheme_opt: Option<&str>,
    cwd: &Path,
) -> Cow<'a, str> {
    let scheme: &str = match scheme_opt {
        Some(s) => s,
        None => return Cow::Borrowed(src),
    };

    CITATION_REGEX.replace_all(src, |caps: &regex_lite::Captures<'_>| {
        let file = &caps[1];
        let start_line = &caps[2];

        // Resolve the path against `cwd` when it is relative.
        let absolute_path = {
            let p = Path::new(file);
            let absolute_path = if p.is_absolute() {
                path_clean::clean(p)
            } else {
                path_clean::clean(cwd.join(p))
            };
            // VS Code expects forward slashes even on Windows because URIs use
            // `/` as the path separator.
            absolute_path.to_string_lossy().replace('\\', "/")
        };

        // Render as a normal markdown link so the downstream renderer emits
        // the hyperlink escape sequence (when supported by the terminal).
        //
        // In practice, sometimes multiple citations for the same file, but with a
        // different line number, are shown sequentially, so we:
        // - include the line number in the label to disambiguate them
        // - add a space after the link to make it easier to read
        format!("[{file}:{start_line}]({scheme}://file{absolute_path}:{start_line}) ")
    })
}

#[cfg(test)]
mod markdown_render_tests {
    include!("markdown_render_tests.rs");
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn citation_is_rewritten_with_absolute_path() {
        let markdown = "See 【F:/src/main.rs†L42-L50】 for details.";
        let cwd = Path::new("/workspace");
        let result = rewrite_file_citations_with_scheme(markdown, Some("vscode"), cwd);

        assert_eq!(
            "See [/src/main.rs:42](vscode://file/src/main.rs:42)  for details.",
            result
        );
    }

    #[test]
    fn citation_followed_by_space_so_they_do_not_run_together() {
        let markdown = "References on lines 【F:src/foo.rs†L24】【F:src/foo.rs†L42】";
        let cwd = Path::new("/home/user/project");
        let result = rewrite_file_citations_with_scheme(markdown, Some("vscode"), cwd);

        assert_eq!(
            "References on lines [src/foo.rs:24](vscode://file/home/user/project/src/foo.rs:24) [src/foo.rs:42](vscode://file/home/user/project/src/foo.rs:42) ",
            result
        );
    }

    #[test]
    fn citation_unchanged_without_file_opener() {
        let markdown = "Look at 【F:file.rs†L1】.";
        let cwd = Path::new("/");
        let unchanged = rewrite_file_citations_with_scheme(markdown, Some("vscode"), cwd);
        // The helper itself always rewrites – this test validates behaviour of
        // append_markdown when `file_opener` is None.
        let rendered = render_markdown_text_with_citations(markdown, None, cwd, None);
        // Convert lines back to string for comparison.
        let rendered: String = rendered
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.clone())
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(markdown, rendered);
        // Ensure helper rewrites.
        assert_ne!(markdown, unchanged);
    }
}
