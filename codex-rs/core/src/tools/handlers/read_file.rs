use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use codex_utils_string::take_bytes_at_char_boundary;
use serde::Deserialize;
use tokio::fs::File;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ReadFileHandler;

const MAX_LINE_LENGTH: usize = 500;
const TAB_WIDTH: usize = 4;

const HEADER_PREFIXES: &[&str] = &["///", "//!", "/**", "/*!", "#[", "#!", "@", "\"\"\"", "'''"];

/// JSON arguments accepted by the `read_file` tool handler.
#[derive(Deserialize)]
struct ReadFileArgs {
    /// Absolute path to the file that will be read.
    file_path: String,
    /// 1-indexed line number to start reading from; defaults to 1.
    #[serde(default = "defaults::offset")]
    offset: usize,
    /// Maximum number of lines to return; defaults to 2000.
    #[serde(default = "defaults::limit")]
    limit: usize,
    /// Determines whether the handler reads a simple slice or indentation-aware block.
    #[serde(default)]
    mode: ReadMode,
    /// Optional indentation configuration used when `mode` is `Indentation`.
    #[serde(default)]
    indentation: Option<IndentationArgs>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReadMode {
    Slice,
    Indentation,
}
/// Additional configuration for indentation-aware reads.
#[derive(Deserialize, Clone)]
struct IndentationArgs {
    /// Optional explicit anchor line; defaults to `offset` when omitted.
    #[serde(default)]
    anchor_line: Option<usize>,
    /// Maximum indentation depth to collect; `0` means unlimited.
    #[serde(default = "defaults::max_levels")]
    max_levels: usize,
    /// Whether to include sibling blocks at the same indentation level.
    #[serde(default = "defaults::include_siblings")]
    include_siblings: bool,
    /// Whether to include header lines above the anchor block. This made on a best effort basis.
    #[serde(default = "defaults::include_header")]
    include_header: bool,
    /// Optional hard cap on returned lines; defaults to the global `limit`.
    #[serde(default)]
    max_lines: Option<usize>,
}

#[derive(Clone, Debug)]
struct LineRecord {
    number: usize,
    raw: String,
    display: String,
    indent: usize,
}

impl LineRecord {
    fn trimmed(&self) -> &str {
        self.raw.trim_start()
    }

    fn is_blank(&self) -> bool {
        self.trimmed().is_empty()
    }

    fn is_header_like(&self) -> bool {
        indentation::is_header_like(self.trimmed())
    }
}

#[async_trait]
impl ToolHandler for ReadFileHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation { payload, .. } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "read_file handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: ReadFileArgs = serde_json::from_str(&arguments).map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to parse function arguments: {err:?}"
            ))
        })?;

        let ReadFileArgs {
            file_path,
            offset,
            limit,
            mode,
            indentation,
        } = args;

        if offset == 0 {
            return Err(FunctionCallError::RespondToModel(
                "offset must be a 1-indexed line number".to_string(),
            ));
        }

        if limit == 0 {
            return Err(FunctionCallError::RespondToModel(
                "limit must be greater than zero".to_string(),
            ));
        }

        let path = PathBuf::from(&file_path);
        if !path.is_absolute() {
            return Err(FunctionCallError::RespondToModel(
                "file_path must be an absolute path".to_string(),
            ));
        }

        let collected = match mode {
            ReadMode::Slice => slice::read(&path, offset, limit).await?,
            ReadMode::Indentation => {
                let indentation = indentation.unwrap_or_default();
                indentation::read_block(&path, offset, limit, indentation).await?
            }
        };
        Ok(ToolOutput::Function {
            content: collected.join("\n"),
            success: Some(true),
        })
    }
}

mod slice {
    use std::path::Path;
    use tokio::fs::File;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use crate::function_tool::FunctionCallError;
    use crate::tools::handlers::read_file::format_line;

    pub async fn read(
        path: &Path,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<String>, FunctionCallError> {
        let file = File::open(path)
            .await
            .map_err(|err| FunctionCallError::RespondToModel(format!("failed to read file: {err}")))?;

        let mut reader = BufReader::new(file);
        let mut collected = Vec::new();
        let mut seen = 0usize;
        let mut buffer = Vec::new();

        loop {
            buffer.clear();
            let bytes_read = reader.read_until(b'\n', &mut buffer).await.map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to read file: {err}"))
            })?;

            if bytes_read == 0 {
                break;
            }

            if buffer.last() == Some(&b'\n') {
                buffer.pop();
                if buffer.last() == Some(&b'\r') {
                    buffer.pop();
                }
            }

            seen += 1;

            if seen < offset {
                continue;
            }

            if collected.len() == limit {
                break;
            }

            let formatted = format_line(&buffer);
            collected.push(format!("L{seen}: {formatted}"));

            if collected.len() == limit {
                break;
            }
        }

        if seen < offset {
            return Err(FunctionCallError::RespondToModel(
                "offset exceeds file length".to_string(),
            ));
        }

        Ok(collected)
    }
}

mod indentation {
    use crate::function_tool::FunctionCallError;
    use crate::tools::handlers::read_file::HEADER_PREFIXES;
    use crate::tools::handlers::read_file::IndentationArgs;
    use crate::tools::handlers::read_file::LineRecord;
    use crate::tools::handlers::read_file::TAB_WIDTH;
    use crate::tools::handlers::read_file::format_line;
    use std::path::Path;
    use tokio::fs::File;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::BufReader;

    pub async fn read_block(
        path: &Path,
        offset: usize,
        limit: usize,
        options: IndentationArgs,
    ) -> Result<Vec<String>, FunctionCallError> {
        let anchor_line = options.anchor_line.unwrap_or(offset);
        if anchor_line == 0 {
            return Err(FunctionCallError::RespondToModel(
                "anchor_line must be a 1-indexed line number".to_string(),
            ));
        }

        let guard_limit = options.max_lines.unwrap_or(limit);
        if guard_limit == 0 {
            return Err(FunctionCallError::RespondToModel(
                "max_lines must be greater than zero".to_string(),
            ));
        }

        let collected = collect_file_lines(path).await?;
        if collected.is_empty() || anchor_line > collected.len() {
            return Err(FunctionCallError::RespondToModel(
                "anchor_line exceeds file length".to_string(),
            ));
        }

        let anchor_index = anchor_line - 1;
        let effective_indents = compute_effective_indents(&collected);
        let anchor_indent = effective_indents[anchor_index];
        let root_indent =
            determine_root_indent(&effective_indents, anchor_index, options.max_levels);
        let start = expand_upwards(
            &collected,
            &effective_indents,
            anchor_index,
            root_indent,
            &options,
        );
        let end = expand_downwards(
            &collected,
            &effective_indents,
            anchor_index,
            root_indent,
            anchor_indent,
            &options,
        );

        let total_span = end - start + 1;
        let mut slice_start = start;
        let mut slice_end = end;
        let mut truncated = false;
        if total_span > guard_limit {
            truncated = true;
            let mut remaining = guard_limit.saturating_sub(1);
            slice_start = anchor_index;
            slice_end = anchor_index;
            while remaining > 0 && (slice_start > start || slice_end < end) {
                if slice_start > start {
                    slice_start -= 1;
                    remaining -= 1;
                }
                if remaining > 0 && slice_end < end {
                    slice_end += 1;
                    remaining -= 1;
                }
                if slice_start == start && slice_end == end {
                    break;
                }
            }
        }

        let mut formatted = Vec::new();
        for record in collected.iter().take(slice_end + 1).skip(slice_start) {
            let mut line = format!("L{}: {}", record.number, record.display);
            if record.number == anchor_line {
                line.push_str(" <- anchor");
            }
            formatted.push(line);
        }

        if truncated {
            formatted.push(format!("... (truncated after {guard_limit} lines)"));
        }

        Ok(formatted)
    }

    async fn collect_file_lines(path: &Path) -> Result<Vec<LineRecord>, FunctionCallError> {
        let file = File::open(path).await.map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to read file: {err}"))
        })?;

        let mut reader = BufReader::new(file);
        let mut buffer = Vec::new();
        let mut lines = Vec::new();
        let mut number = 0usize;

        loop {
            buffer.clear();
            let bytes_read = reader.read_until(b'\n', &mut buffer).await.map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to read file: {err}"))
            })?;

            if bytes_read == 0 {
                break;
            }

            if buffer.last() == Some(&b'\n') {
                buffer.pop();
                if buffer.last() == Some(&b'\r') {
                    buffer.pop();
                }
            }

            number += 1;
            let raw = String::from_utf8_lossy(&buffer).into_owned();
            let indent = measure_indent(&raw);
            let display = format_line(&buffer);
            lines.push(LineRecord {
                number,
                raw,
                display,
                indent,
            });
        }

        Ok(lines)
    }

    fn compute_effective_indents(records: &[LineRecord]) -> Vec<usize> {
        let mut effective = Vec::with_capacity(records.len());
        let mut previous_indent = 0usize;
        for record in records {
            if record.is_blank() {
                effective.push(previous_indent);
            } else {
                previous_indent = record.indent;
                effective.push(previous_indent);
            }
        }
        effective
    }

    fn determine_root_indent(effective: &[usize], anchor_index: usize, max_levels: usize) -> usize {
        let mut root = effective[anchor_index];
        let mut remaining = max_levels.saturating_add(1);
        let mut index = anchor_index;
        while index > 0 && remaining > 0 {
            index -= 1;
            if effective[index] < root {
                root = effective[index];
                remaining -= 1;
            }
        }
        root
    }

    fn expand_upwards(
        records: &[LineRecord],
        effective: &[usize],
        anchor_index: usize,
        root_indent: usize,
        options: &IndentationArgs,
    ) -> usize {
        let mut index = anchor_index;
        let mut header_chain = false;
        while index > 0 {
            let candidate = index - 1;
            let record = &records[candidate];
            let indent = effective[candidate];
            let header_like = record.is_header_like();
            let include = if record.is_blank() {
                header_chain || indent >= root_indent
            } else if header_like {
                options.include_header
            } else {
                indent >= root_indent
            };

            if include {
                index -= 1;
                if header_like && options.include_header {
                    header_chain = true;
                } else if !record.is_blank() {
                    header_chain = false;
                }
                continue;
            }
            break;
        }
        index
    }

    fn expand_downwards(
        records: &[LineRecord],
        effective: &[usize],
        anchor_index: usize,
        root_indent: usize,
        anchor_indent: usize,
        options: &IndentationArgs,
    ) -> usize {
        let mut end = anchor_index;
        let mut stack = vec![root_indent];
        if anchor_indent > root_indent {
            stack.push(anchor_indent);
        }
        let mut anchor_active = true;

        for (idx, record) in records.iter().enumerate().skip(anchor_index + 1) {
            let indent = effective[idx];
            if indent < root_indent && !(options.include_header && record.is_header_like()) {
                break;
            }

            while indent < *stack.last().unwrap_or(&root_indent) {
                let popped = stack.pop().unwrap_or(root_indent);
                if popped == anchor_indent {
                    anchor_active = false;
                }
                if stack.is_empty() {
                    stack.push(root_indent);
                    break;
                }
            }

            if indent > *stack.last().unwrap_or(&root_indent) {
                stack.push(indent);
            }

            if indent == anchor_indent {
                anchor_active = true;
            }

            let closing_line = is_closing_line(record.trimmed());
            if !options.include_siblings && !anchor_active && !record.is_blank() && !closing_line {
                break;
            }

            end = idx;
        }

        end
    }

    pub fn is_header_like(trimmed: &str) -> bool {
        for prefix in HEADER_PREFIXES {
            if trimmed.starts_with(prefix) {
                return true;
            }
        }

        if trimmed.starts_with('#') && trimmed.len() > 1 {
            return matches!(trimmed.as_bytes()[1], b'[' | b'!');
        }

        false
    }

    fn measure_indent(line: &str) -> usize {
        line.chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .map(|c| if c == '\t' { TAB_WIDTH } else { 1 })
            .sum()
    }

    fn is_closing_line(trimmed: &str) -> bool {
        match trimmed.chars().next() {
            Some('}') | Some(']') | Some(')') => true,
            Some(_) => trimmed.starts_with("end ") || trimmed == "end",
            None => false,
        }
    }
}

fn format_line(bytes: &[u8]) -> String {
    let decoded = String::from_utf8_lossy(bytes);
    if decoded.len() > MAX_LINE_LENGTH {
        take_bytes_at_char_boundary(&decoded, MAX_LINE_LENGTH).to_string()
    } else {
        decoded.into_owned()
    }
}

mod defaults {
    use super::*;

    impl Default for IndentationArgs {
        fn default() -> Self {
            Self {
                anchor_line: None,
                max_levels: max_levels(),
                include_siblings: include_siblings(),
                include_header: include_header(),
                max_lines: None,
            }
        }
    }

    impl Default for ReadMode {
        fn default() -> Self {
            Self::Slice
        }
    }

    pub fn offset() -> usize {
        1
    }

    pub fn limit() -> usize {
        2000
    }

    pub fn max_levels() -> usize {
        0
    }

    pub fn include_siblings() -> bool {
        false
    }

    pub fn include_header() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::indentation::read_block;
    use super::slice::read;
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn reads_requested_range() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        writeln!(temp, "alpha")?;
        writeln!(temp, "beta")?;
        writeln!(temp, "gamma")?;

        let lines = read(temp.path(), 2, 2).await?;
        assert_eq!(lines, vec!["L2: beta".to_string(), "L3: gamma".to_string()]);
        Ok(())
    }

    #[tokio::test]
    async fn errors_when_offset_exceeds_length() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        writeln!(temp, "only")?;

        let err = read(temp.path(), 3, 1)
            .await
            .expect_err("offset exceeds length");
        assert_eq!(
            err,
            FunctionCallError::RespondToModel("offset exceeds file length".to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn reads_non_utf8_lines() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        temp.as_file_mut().write_all(b"\xff\xfe\nplain\n")?;

        let lines = read(temp.path(), 1, 2).await?;
        let expected_first = format!("L1: {}{}", '\u{FFFD}', '\u{FFFD}');
        assert_eq!(lines, vec![expected_first, "L2: plain".to_string()]);
        Ok(())
    }

    #[tokio::test]
    async fn trims_crlf_endings() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        write!(temp, "one\r\ntwo\r\n")?;

        let lines = read(temp.path(), 1, 2).await?;
        assert_eq!(lines, vec!["L1: one".to_string(), "L2: two".to_string()]);
        Ok(())
    }

    #[tokio::test]
    async fn respects_limit_even_with_more_lines() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        writeln!(temp, "first")?;
        writeln!(temp, "second")?;
        writeln!(temp, "third")?;

        let lines = read(temp.path(), 1, 2).await?;
        assert_eq!(
            lines,
            vec!["L1: first".to_string(), "L2: second".to_string()]
        );
        Ok(())
    }

    #[tokio::test]
    async fn truncates_lines_longer_than_max_length() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        let long_line = "x".repeat(MAX_LINE_LENGTH + 50);
        writeln!(temp, "{long_line}")?;

        let lines = read(temp.path(), 1, 1).await?;
        let expected = "x".repeat(MAX_LINE_LENGTH);
        assert_eq!(lines, vec![format!("L1: {expected}")]);
        Ok(())
    }

    #[tokio::test]
    async fn indentation_mode_captures_block() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        writeln!(temp, "fn outer() {{")?;
        writeln!(temp, "    if cond {{")?;
        writeln!(temp, "        inner();")?;
        writeln!(temp, "    }}")?;
        writeln!(temp, "    tail();")?;
        writeln!(temp, "}}")?;

        let mut options = IndentationArgs::default();
        options.anchor_line = Some(3);
        options.include_siblings = false;

        let lines = read_block(temp.path(), 3, 10, options).await?;

        assert_eq!(
            lines,
            vec![
                "L2:     if cond {".to_string(),
                "L3:         inner(); <- anchor".to_string(),
                "L4:     }".to_string()
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn indentation_mode_expands_parents() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        writeln!(temp, "mod root {{")?;
        writeln!(temp, "    fn outer() {{")?;
        writeln!(temp, "        if cond {{")?;
        writeln!(temp, "            inner();")?;
        writeln!(temp, "        }}")?;
        writeln!(temp, "    }}")?;
        writeln!(temp, "}}")?;

        let mut options = IndentationArgs::default();
        options.anchor_line = Some(4);
        options.max_levels = 1;

        let lines = read_block(temp.path(), 4, 50, options.clone()).await?;
        assert_eq!(
            lines,
            vec![
                "L2:     fn outer() {".to_string(),
                "L3:         if cond {".to_string(),
                "L4:             inner(); <- anchor".to_string(),
                "L5:         }".to_string(),
                "L6:     }".to_string(),
            ]
        );

        options.max_levels = 2;
        let expanded = read_block(temp.path(), 4, 50, options).await?;
        assert_eq!(
            expanded,
            vec![
                "L1: mod root {".to_string(),
                "L2:     fn outer() {".to_string(),
                "L3:         if cond {".to_string(),
                "L4:             inner(); <- anchor".to_string(),
                "L5:         }".to_string(),
                "L6:     }".to_string(),
                "L7: }".to_string(),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn indentation_mode_truncates_with_guard() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        writeln!(temp, "fn sample() {{")?;
        for _ in 0..20 {
            writeln!(temp, "    body_line();")?;
        }
        writeln!(temp, "}}")?;

        let mut options = IndentationArgs::default();
        options.anchor_line = Some(5);
        options.max_lines = Some(5);

        let lines = read_block(temp.path(), 5, 100, options).await?;

        assert_eq!(lines.len(), 6);
        assert!(lines.iter().any(|line| line.contains("<- anchor")));
        assert!(lines.last().unwrap().contains("truncated"));
        Ok(())
    }

    #[tokio::test]
    async fn indentation_mode_respects_sibling_flag() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        writeln!(temp, "fn wrapper() {{")?;
        writeln!(temp, "    if first {{")?;
        writeln!(temp, "        do_first();")?;
        writeln!(temp, "    }}")?;
        writeln!(temp, "    if second {{")?;
        writeln!(temp, "        do_second();")?;
        writeln!(temp, "    }}")?;
        writeln!(temp, "}}")?;

        let mut options = IndentationArgs::default();
        options.anchor_line = Some(3);
        options.include_siblings = false;

        let lines = read_block(temp.path(), 3, 50, options.clone()).await?;
        assert_eq!(
            lines,
            vec![
                "L2:     if first {".to_string(),
                "L3:         do_first(); <- anchor".to_string(),
                "L4:     }".to_string(),
            ]
        );

        options.include_siblings = true;
        let with_siblings = read_block(temp.path(), 3, 50, options).await?;
        assert_eq!(
            with_siblings,
            vec![
                "L2:     if first {".to_string(),
                "L3:         do_first(); <- anchor".to_string(),
                "L4:     }".to_string(),
                "L5:     if second {".to_string(),
                "L6:         do_second();".to_string(),
                "L7:     }".to_string(),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn indentation_mode_handles_python_sample() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        writeln!(temp, "class Foo:")?;
        writeln!(temp, "    def __init__(self, size):")?;
        writeln!(temp, "        self.size = size")?;
        writeln!(temp, "    def double(self, value):")?;
        writeln!(temp, "        if value is None:")?;
        writeln!(temp, "            return 0")?;
        writeln!(temp, "        result = value * self.size")?;
        writeln!(temp, "        return result")?;
        writeln!(temp, "class Bar:")?;
        writeln!(temp, "    def compute(self):")?;
        writeln!(temp, "        helper = Foo(2)")?;
        writeln!(temp, "        return helper.double(5)")?;

        let mut options = IndentationArgs::default();
        options.anchor_line = Some(7);

        let lines = read_block(temp.path(), 7, 200, options).await?;
        assert_eq!(
            lines,
            vec![
                "L2:     def __init__(self, size):".to_string(),
                "L3:         self.size = size".to_string(),
                "L4:     def double(self, value):".to_string(),
                "L5:         if value is None:".to_string(),
                "L6:             return 0".to_string(),
                "L7:         result = value * self.size <- anchor".to_string(),
                "L8:         return result".to_string(),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn indentation_mode_handles_javascript_sample() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        writeln!(temp, "export function makeThing() {{")?;
        writeln!(temp, "    const cache = new Map();")?;
        writeln!(temp, "    function ensure(key) {{")?;
        writeln!(temp, "        if (!cache.has(key)) {{")?;
        writeln!(temp, "            cache.set(key, []);")?;
        writeln!(temp, "        }}")?;
        writeln!(temp, "        return cache.get(key);")?;
        writeln!(temp, "    }}")?;
        writeln!(temp, "    const handlers = {{")?;
        writeln!(temp, "        init() {{")?;
        writeln!(temp, "            console.log(\"init\");")?;
        writeln!(temp, "        }},")?;
        writeln!(temp, "        run() {{")?;
        writeln!(temp, "            if (Math.random() > 0.5) {{")?;
        writeln!(temp, "                return \"heads\";")?;
        writeln!(temp, "            }}")?;
        writeln!(temp, "            return \"tails\";")?;
        writeln!(temp, "        }},")?;
        writeln!(temp, "    }};")?;
        writeln!(temp, "    return {{ cache, handlers }};")?;
        writeln!(temp, "}}")?;
        writeln!(temp, "export function other() {{")?;
        writeln!(temp, "    return makeThing();")?;
        writeln!(temp, "}}")?;

        let mut options = IndentationArgs::default();
        options.anchor_line = Some(15);
        options.max_levels = 1;

        let lines = read_block(temp.path(), 15, 200, options).await?;
        assert_eq!(
            lines,
            vec![
                "L10:         init() {".to_string(),
                "L11:             console.log(\"init\");".to_string(),
                "L12:         },".to_string(),
                "L13:         run() {".to_string(),
                "L14:             if (Math.random() > 0.5) {".to_string(),
                "L15:                 return \"heads\"; <- anchor".to_string(),
                "L16:             }".to_string(),
                "L17:             return \"tails\";".to_string(),
                "L18:         },".to_string(),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn indentation_mode_handles_cpp_sample() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        writeln!(temp, "#include <vector>")?;
        writeln!(temp, "#include <string>")?;
        writeln!(temp, "")?;
        writeln!(temp, "namespace sample {{")?;
        writeln!(temp, "class Runner {{")?;
        writeln!(temp, "public:")?;
        writeln!(temp, "    void setup() {{")?;
        writeln!(temp, "        if (enabled_) {{")?;
        writeln!(temp, "            init();")?;
        writeln!(temp, "        }}")?;
        writeln!(temp, "    }}")?;
        writeln!(temp, "")?;
        writeln!(temp, "    // Run the code")?;
        writeln!(temp, "    int run() const {{")?;
        writeln!(temp, "        switch (mode_) {{")?;
        writeln!(temp, "            case Mode::Fast:")?;
        writeln!(temp, "                return fast();")?;
        writeln!(temp, "            case Mode::Slow:")?;
        writeln!(temp, "                return slow();")?;
        writeln!(temp, "            default:")?;
        writeln!(temp, "                return fallback();")?;
        writeln!(temp, "        }}")?;
        writeln!(temp, "    }}")?;
        writeln!(temp, "")?;
        writeln!(temp, "private:")?;
        writeln!(temp, "    bool enabled_ = false;")?;
        writeln!(temp, "    Mode mode_ = Mode::Fast;")?;
        writeln!(temp, "")?;
        writeln!(temp, "    int fast() const {{")?;
        writeln!(temp, "        return 1;")?;
        writeln!(temp, "    }}")?;
        writeln!(temp, "}};")?;
        writeln!(temp, "}}  // namespace sample")?;

        let mut options = IndentationArgs::default();
        options.include_siblings = false;
        options.anchor_line = Some(18);
        options.max_levels = 2;

        let lines = read_block(temp.path(), 18, 200, options).await?;
        assert_eq!(
            lines,
            vec![
                "L14:         switch (mode_) {".to_string(),
                "L15:             case Mode::Fast:".to_string(),
                "L16:                 return fast();".to_string(),
                "L17:             case Mode::Slow:".to_string(),
                "L18:                 return slow(); <- anchor".to_string(),
                "L19:             default:".to_string(),
                "L20:                 return fallback();".to_string(),
                "L21:         }".to_string(),
            ]
        );
        Ok(())
    }
}
