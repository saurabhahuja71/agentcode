use anyhow::Result;
use crate::theme::THEME;
use forge_core::{
    Agent, AgentEvent, ApprovalGate, Interactivity, OptionsGate, Session, SessionStore, TodoItem,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};
use std::cell::Cell;
use std::collections::VecDeque;
use std::io;
use std::time::Instant;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

const TOOL_CARD_COLLAPSE_THRESHOLD: usize = 6;
const SPINNER_FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + 2);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Selection-aware span push: splits `text` into selected/unselected segments
/// so copy ranges stay byte-accurate while styling differs.
fn push_segment(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    fg: Color,
    bg: Option<Color>,
    modifier: Modifier,
    abs_start: usize,
    sel_start: usize,
    sel_end: usize,
) {
    let abs_end = abs_start + text.len();
    let has_selection = sel_start != sel_end && abs_start < sel_end && abs_end > sel_start;

    if !has_selection {
        let mut style = Style::default().fg(fg).add_modifier(modifier);
        if let Some(bg) = bg {
            style = style.bg(bg);
        }
        spans.push(Span::styled(text.to_string(), style));
        return;
    }

    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut current_segment = String::new();
    let mut segment_selected = false;

    for (idx, (byte_idx, c)) in chars.iter().enumerate() {
        let char_abs = abs_start + byte_idx;
        let is_char_selected = char_abs >= sel_start && char_abs < sel_end;

        if idx == 0 {
            segment_selected = is_char_selected;
            current_segment.push(*c);
        } else if is_char_selected == segment_selected {
            current_segment.push(*c);
        } else {
            let style = if segment_selected {
                Style::default()
                    .bg(THEME.selection_bg)
                    .fg(THEME.selection_fg)
                    .add_modifier(modifier)
            } else {
                Style::default().fg(fg).add_modifier(modifier)
            };
            spans.push(Span::styled(current_segment, style));
            segment_selected = is_char_selected;
            current_segment = c.to_string();
        }
    }
    if !current_segment.is_empty() {
        let style = if segment_selected {
            Style::default()
                .bg(THEME.selection_bg)
                .fg(THEME.selection_fg)
                .add_modifier(modifier)
        } else {
            Style::default().fg(fg).add_modifier(modifier)
        };
        spans.push(Span::styled(current_segment, style));
    }
}

/// Apply a background to every span on a line, preserving the selection highlight.
fn add_line_bg(line: &mut Line<'static>, bg: Color) {
    for span in line.spans.iter_mut() {
        if span.style.bg == Some(THEME.selection_bg) {
            continue;
        }
        span.style = span.style.bg(bg);
    }
}

/// Code-fence body: light token tinting (strings, comments, numbers).
fn render_code_line(
    line_str: &str,
    base_idx: usize,
    sel_start: usize,
    sel_end: usize,
) -> Line<'static> {
    let mut spans = Vec::new();
    let chars: Vec<(usize, char)> = line_str.char_indices().collect();
    let mut i = 0;
    let mut seg_start = 0;
    let mut comment_rest = false;

    let push = |spans: &mut Vec<Span<'static>>, text: &str, fg: Color, rel: usize| {
        push_segment(
            spans,
            text,
            fg,
            None,
            Modifier::empty(),
            base_idx + rel,
            sel_start,
            sel_end,
        );
    };

    while i < chars.len() {
        if comment_rest {
            break;
        }
        let (b, c) = chars[i];
        let next = chars.get(i + 1).map(|(_, n)| *n);

        if (c == '/' && next == Some('/')) || (c == '-' && next == Some('-')) || c == '#' {
            if b > seg_start {
                push(&mut spans, &line_str[seg_start..b], THEME.code_fg, seg_start);
            }
            push(&mut spans, &line_str[b..], THEME.code_comment, b);
            comment_rest = true;
            break;
        }

        if c == '"' || c == '\'' {
            if b > seg_start {
                push(&mut spans, &line_str[seg_start..b], THEME.code_fg, seg_start);
            }
            let mut end = i + 1;
            let mut closed = false;
            while end < chars.len() {
                if chars[end].1 == '\\' {
                    end += 2;
                    continue;
                }
                if chars[end].1 == c {
                    closed = true;
                    break;
                }
                end += 1;
            }
            if closed {
                let content_end_byte = chars[end].0 + 1;
                push(
                    &mut spans,
                    &line_str[b..content_end_byte],
                    THEME.code_string,
                    b,
                );
                i = end + 1;
                seg_start = content_end_byte;
                continue;
            }
        }

        if c.is_ascii_digit() {
            if b > seg_start {
                push(&mut spans, &line_str[seg_start..b], THEME.code_fg, seg_start);
            }
            let mut end = i + 1;
            while end < chars.len()
                && (chars[end].1.is_ascii_alphanumeric()
                    || chars[end].1 == '.'
                    || chars[end].1 == '_')
            {
                end += 1;
            }
            let num_end_byte = chars[end - 1].0 + chars[end - 1].1.len_utf8();
            push(
                &mut spans,
                &line_str[b..num_end_byte],
                THEME.code_number,
                b,
            );
            i = end;
            seg_start = num_end_byte;
            continue;
        }

        i += 1;
    }

    if !comment_rest && seg_start < line_str.len() {
        push(&mut spans, &line_str[seg_start..], THEME.code_fg, seg_start);
    }

    Line::from(spans)
}

fn parse_line_markdown(
    line_str: &str,
    base_idx: usize,
    sel_start: usize,
    sel_end: usize,
    inside_code_block: bool,
    is_indicator: bool,
    default_fg: Color,
) -> Line<'static> {
    let mut spans = Vec::new();
    let is_header = line_str.starts_with("#");
    let is_prompt = line_str.starts_with("> ");
    let is_tool = line_str.starts_with("▾ ") || line_str.starts_with("▸ ");

    let line_fg = if is_indicator {
        default_fg
    } else if inside_code_block {
        THEME.code_fg
    } else if is_header || is_prompt {
        THEME.text_accent
    } else {
        THEME.text
    };

    let default_modifier = if is_header || is_prompt {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };

    let mut append_styled = |text: &str,
                             fg: Color,
                             bg: Option<Color>,
                             modifier: Modifier,
                             start_rel_offset: usize| {
        push_segment(
            &mut spans,
            text,
            fg,
            bg,
            modifier,
            base_idx + start_rel_offset,
            sel_start,
            sel_end,
        );
    };

    if is_indicator {
        append_styled(line_str, default_fg, None, Modifier::empty(), 0);
    } else if inside_code_block {
        return render_code_line(line_str, base_idx, sel_start, sel_end);
    } else if is_header || is_prompt || is_tool {
        append_styled(line_str, THEME.text_accent, None, Modifier::BOLD, 0);
    } else {
        let mut display_str = line_str;
        let mut offset = 0;

        let trimmed = line_str.trim_start();
        let leading_whitespace_len = line_str.len() - trimmed.len();

        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            if leading_whitespace_len > 0 {
                append_styled(
                    &line_str[..leading_whitespace_len],
                    THEME.text,
                    None,
                    Modifier::empty(),
                    0,
                );
            }
            append_styled(
                "• ",
                THEME.text_accent,
                None,
                Modifier::BOLD,
                leading_whitespace_len,
            );
            display_str = &trimmed[2..];
            offset = leading_whitespace_len + 2;
        }

        let chars: Vec<(usize, char)> = display_str.char_indices().collect();
        let mut i = 0;
        let mut normal_start_idx = 0;

        while i < chars.len() {
            if i + 1 < chars.len() && chars[i].1 == '*' && chars[i + 1].1 == '*' {
                if chars[i].0 > normal_start_idx {
                    append_styled(
                        &display_str[normal_start_idx..chars[i].0],
                        line_fg,
                        None,
                        default_modifier,
                        offset + normal_start_idx,
                    );
                }

                let mut bold_end_char_idx = i + 2;
                let mut found_bold_close = false;
                while bold_end_char_idx + 1 < chars.len() {
                    if chars[bold_end_char_idx].1 == '*'
                        && chars[bold_end_char_idx + 1].1 == '*'
                    {
                        found_bold_close = true;
                        break;
                    }
                    bold_end_char_idx += 1;
                }

                if found_bold_close {
                    let content_start = chars[i + 2].0;
                    let content_end = chars[bold_end_char_idx].0;
                    append_styled(
                        &display_str[content_start..content_end],
                        THEME.warning,
                        None,
                        Modifier::BOLD,
                        offset + content_start,
                    );
                    i = bold_end_char_idx + 2;
                    normal_start_idx =
                        if i < chars.len() { chars[i].0 } else { display_str.len() };
                    continue;
                }
            }

            if chars[i].1 == '*' && !(i + 1 < chars.len() && chars[i + 1].1 == '*') {
                let mut close = i + 1;
                let mut found_close = false;
                while close < chars.len() {
                    if chars[close].1 == '*'
                        && !(close + 1 < chars.len() && chars[close + 1].1 == '*')
                    {
                        found_close = true;
                        break;
                    }
                    close += 1;
                }

                if found_close && close > i + 1 {
                    if chars[i].0 > normal_start_idx {
                        append_styled(
                            &display_str[normal_start_idx..chars[i].0],
                            line_fg,
                            None,
                            default_modifier,
                            offset + normal_start_idx,
                        );
                    }
                    let content_start = chars[i + 1].0;
                    let content_end = chars[close].0;
                    append_styled(
                        &display_str[content_start..content_end],
                        line_fg,
                        None,
                        Modifier::ITALIC,
                        offset + content_start,
                    );
                    i = close + 1;
                    normal_start_idx =
                        if i < chars.len() { chars[i].0 } else { display_str.len() };
                    continue;
                }
            }

            if chars[i].1 == '`' {
                if chars[i].0 > normal_start_idx {
                    append_styled(
                        &display_str[normal_start_idx..chars[i].0],
                        line_fg,
                        None,
                        default_modifier,
                        offset + normal_start_idx,
                    );
                }

                let mut code_end_char_idx = i + 1;
                let mut found_code_close = false;
                while code_end_char_idx < chars.len() {
                    if chars[code_end_char_idx].1 == '`' {
                        found_code_close = true;
                        break;
                    }
                    code_end_char_idx += 1;
                }

                if found_code_close {
                    let content_start = chars[i + 1].0;
                    let content_end = chars[code_end_char_idx].0;
                    append_styled(
                        &display_str[content_start..content_end],
                        THEME.code_fg,
                        Some(THEME.code_bg),
                        Modifier::empty(),
                        offset + content_start,
                    );
                    i = code_end_char_idx + 1;
                    normal_start_idx =
                        if i < chars.len() { chars[i].0 } else { display_str.len() };
                    continue;
                }
            }

            i += 1;
        }

        if normal_start_idx < display_str.len() {
            append_styled(
                &display_str[normal_start_idx..],
                line_fg,
                None,
                default_modifier,
                offset + normal_start_idx,
            );
        }
    }

    Line::from(spans)
}

fn wrap_text(text: &str, width: usize) -> Vec<std::ops::Range<usize>> {
    let mut lines = Vec::new();
    let mut start_idx = 0;
    
    for line in text.split('\n') {
        let line_len = line.len();
        if line_len == 0 {
            lines.push(start_idx..start_idx);
            start_idx += 1; // account for \n
            continue;
        }
        
        let chars: Vec<(usize, char)> = line.char_indices().collect();
        let mut i = 0;
        
        while i < chars.len() {
            let mut word_end_char_idx = i;
            let mut width_so_far = 0;
            let mut last_space_char_idx = None;
            
            while word_end_char_idx < chars.len() {
                let (_, c) = chars[word_end_char_idx];
                let c_width = if c == '\t' { 4 } else { 1 };
                if width_so_far + c_width > width {
                    break;
                }
                width_so_far += c_width;
                if c == ' ' {
                    last_space_char_idx = Some(word_end_char_idx);
                }
                word_end_char_idx += 1;
            }
            
            let break_char_idx = if word_end_char_idx == chars.len() {
                word_end_char_idx
            } else if let Some(space_idx) = last_space_char_idx {
                space_idx + 1
            } else {
                word_end_char_idx
            };
            
            let break_char_idx = if break_char_idx <= i {
                i + 1
            } else {
                break_char_idx
            };
            
            let byte_start = start_idx + chars[i].0;
            let byte_end = if break_char_idx < chars.len() {
                start_idx + chars[break_char_idx].0
            } else {
                start_idx + line_len
            };
            
            lines.push(byte_start..byte_end);
            i = break_char_idx;
        }
        
        start_idx += line_len + 1; // account for \n
    }
    lines
}

fn write_clipboard(text: &str, app: &mut TuiApp) -> bool {
    app.yank_buffer = text.to_string();
    
    let local_res = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text.to_string()));
    let osc_res = write_osc52(text);
    
    local_res.is_ok() || osc_res.is_ok()
}

fn write_osc52(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    
    let encoded = STANDARD.encode(text);
    let osc_sequence = format!("\x1B]52;c;{}\x07", encoded);
    
    let mut stdout = std::io::stdout();
    stdout.write_all(osc_sequence.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

fn clean_copied_text(text: &str) -> String {
    let re_ansi = regex::Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap();
    let cleaned = re_ansi.replace_all(text, "").into_owned();
    
    let mut lines = Vec::new();
    for line in cleaned.lines() {
        let mut l = line;
        if l.starts_with("⊘ Running ") {
            l = &l["⊘ Running ".len()..];
        } else if l.starts_with("▾ ") {
            l = &l["▾ ".len()..];
        } else if l.starts_with("▸ ") {
            l = &l["▸ ".len()..];
        } else if l.starts_with("✔ ") {
            l = &l["✔ ".len()..];
        } else if l.starts_with("❌ ") {
            l = &l["❌ ".len()..];
        } else if l.starts_with("> ") {
            l = &l["> ".len()..];
        }
        lines.push(l);
    }
    
    lines.join("\n")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickType {
    Single,
    Double,
    Triple,
}

/// A tool call rendered as a card in the transcript. The body (result) can be
/// collapsed to a single line; the header glyph shows ▾ expanded / ▸ collapsed.
#[derive(Debug, Clone)]
struct ToolCard {
    /// Byte offset of the header glyph in `output_text`.
    header_start: usize,
    /// Byte offset just past the header text (before its trailing newline).
    header_end: usize,
    /// Byte offset where the body (result) begins.
    body_start: usize,
    /// Byte offset just past the body (exclusive).
    body_end: usize,
    collapsed: bool,
    is_error: bool,
    completed: bool,
}

/// Folded view region: a collapsed body replaced by a placeholder line.
/// `[folded_start, folded_end)` in the folded string corresponds to
/// `[orig_start, orig_end)` in `output_text`.
#[derive(Debug, Clone, Copy)]
struct FoldRegion {
    folded_start: usize,
    folded_end: usize,
    orig_start: usize,
    orig_end: usize,
}

#[derive(Debug, Clone)]
struct PendingApproval {
    tool_name: String,
    arguments: String,
}

/// An in-flight options prompt: the agent asked the user to pick a choice.
#[derive(Debug, Clone)]
struct OptionsState {
    prompt: String,
    options: Vec<String>,
    selected: usize,
    custom: String,
}

impl OptionsState {
    fn text(&self) -> String {
        if self.selected < self.options.len() {
            self.options[self.selected].clone()
        } else {
            self.custom.trim().to_string()
        }
    }
}

pub struct TuiApp {
    pub input: String,
    /// Plain-text output log rendered with word wrap.
    output_text: String,
    pub status: String,
    pub model: String,
    pub provider: String,
    pub session_id: String,
    pub show_help: bool,
    available_models: Vec<String>,
    show_model_picker: bool,
    model_picker_index: usize,

    // NEW FIELDS FOR TEXT SELECTION AND MOUSE
    pub last_output_area: std::cell::Cell<Rect>,
    pub selection: Option<(usize, usize)>,
    pub is_selecting_mouse: bool,
    pub double_click_state: Option<(Instant, usize)>,
    pub last_click_type: ClickType,
    pub yank_buffer: String,
    pub mouse_enabled: bool,
    pub input_cursor_idx: usize,
    pub input_selection_start: Option<usize>,
    pub scroll_offset: Option<u16>,

    // PHASE 2: streaming chat / conversation
    pub input_history: Vec<String>,
    history_index: Option<usize>,
    pub frame: u64,
    tool_cards: Vec<ToolCard>,
    pending_tool_cards: VecDeque<usize>,
    pending_approval: Option<PendingApproval>,
    pub streaming: bool,

    // PHASE 3: options picker, todo panel, thought blocks, rich status bar
    pending_options: Option<OptionsState>,
    todos: Vec<TodoItem>,
    show_todo_panel: bool,
    todo_panel_pct: u16,
    todo_focus: bool,
    todo_selected: usize,
    current_thought: String,
    thinking_start: Option<Instant>,
    tokens_total: u64,
    last_usage: Option<(u32, u32, u32)>,
    context_window: usize,
    permission_mode: String,
    /// (input, output) price per 1M tokens for the current model, when known.
    model_price: Option<(f64, f64)>,
    /// Accumulated session cost in USD.
    total_cost: f64,
    side_area: Cell<Rect>,
    options_area: Cell<Rect>,
}

struct InFlight {
    handle: JoinHandle<(Session, Result<()>)>,
    rx: UnboundedReceiver<AgentEvent>,
}

/// UI -> agent channels for a running turn (approval + options picker).
struct UiChannels<'a> {
    approval: &'a mut Option<UnboundedSender<bool>>,
    options: &'a mut Option<UnboundedSender<String>>,
}

impl TuiApp {
    pub fn new(session: &Session, available_models: Vec<String>) -> Self {
        let model_picker_index = available_models
            .iter()
            .position(|m| m == &session.model)
            .unwrap_or(0);
        Self {
            input: String::new(),
            output_text: "Forge ready. Type a message or /help for commands.\n".into(),
            status: "idle".into(),
            model: session.model.clone(),
            provider: "unknown".into(),
            session_id: session.id.clone(),
            show_help: false,
            available_models,
            show_model_picker: false,
            model_picker_index,
            last_output_area: std::cell::Cell::new(Rect::default()),
            selection: None,
            is_selecting_mouse: false,
            double_click_state: None,
            last_click_type: ClickType::Single,
            yank_buffer: String::new(),
            mouse_enabled: true,
            input_cursor_idx: 0,
            input_selection_start: None,
            scroll_offset: None,
            input_history: Vec::new(),
            history_index: None,
            frame: 0,
            tool_cards: Vec::new(),
            pending_tool_cards: VecDeque::new(),
            pending_approval: None,
            streaming: false,
            pending_options: None,
            todos: session.todos.clone(),
            show_todo_panel: true,
            todo_panel_pct: 30,
            todo_focus: false,
            todo_selected: 0,
            current_thought: String::new(),
            thinking_start: None,
            tokens_total: 0,
            last_usage: None,
            context_window: 0,
            permission_mode: "ask".into(),
            model_price: None,
            total_cost: 0.0,
            side_area: std::cell::Cell::new(Rect::default()),
            options_area: std::cell::Cell::new(Rect::default()),
        }
    }

    fn ensure_newline(&mut self) {
        if !self.output_text.is_empty() && !self.output_text.ends_with('\n') {
            self.output_text.push('\n');
        }
    }

    fn normalize_trailing_newlines(&mut self) {
        self.output_text = self.output_text.trim_end().to_string();
        self.output_text.push('\n');
    }

    pub fn push_output(&mut self, text: &str, _style: Style) {
        if text.is_empty() {
            return;
        }
        self.ensure_newline();
        self.output_text.push_str(text);
        self.normalize_trailing_newlines();
        self.trim_output_if_needed();
    }

    pub fn append_stream(&mut self, text: &str, _style: Style) {
        if text.is_empty() {
            return;
        }
        self.output_text.push_str(text);
        self.trim_output_if_needed();
    }

    /// Keep the output buffer bounded so wrap/scroll stays correct and fast
    /// for long interactive sessions.
    fn trim_output_if_needed(&mut self) {
        const MAX_OUTPUT_CHARS: usize = 200_000;
        if self.output_text.len() <= MAX_OUTPUT_CHARS {
            return;
        }
        let excess = self.output_text.len() - MAX_OUTPUT_CHARS;
        // Drop whole lines when possible so we don't start mid-glyph/line.
        let cut = self.output_text[excess..]
            .find('\n')
            .map(|i| excess + i + 1)
            .unwrap_or(excess);
        self.output_text = self.output_text[cut..].to_string();
        // Byte offsets of tool cards / selection are no longer valid after a trim.
        self.tool_cards.clear();
        self.pending_tool_cards.clear();
        self.selection = None;
    }

    pub fn end_stream(&mut self) {
        self.normalize_trailing_newlines();
    }

    fn paste_into_input(&mut self, text: &str) {
        self.delete_selected_input();
        let mut chars: Vec<char> = self.input.chars().collect();
        for (i, ch) in text.chars().enumerate() {
            chars.insert(self.input_cursor_idx + i, ch);
        }
        self.input_cursor_idx += text.chars().count();
        self.input = chars.into_iter().collect();
    }

    fn copy_output(&mut self) -> bool {
        let cleaned = clean_copied_text(&self.output_text);
        if write_clipboard(&cleaned, self) {
            self.status = "copied output".into();
            true
        } else {
            self.status = "clipboard unavailable".into();
            false
        }
    }

    pub fn copy_selection(&mut self) -> bool {
        if let Some((s, e)) = self.selection {
            let start = s.min(e);
            let end = s.max(e);
            if start < end {
                let selected_raw = &self.output_text[start..end];
                let cleaned = clean_copied_text(selected_raw);
                if !cleaned.is_empty() {
                    if write_clipboard(&cleaned, self) {
                        self.status = "copied selection".into();
                        return true;
                    }
                }
            }
        }
        false
    }

    fn paste_from_clipboard(&mut self) -> bool {
        let local_text = arboard::Clipboard::new().ok().and_then(|mut cb| cb.get_text().ok());
        if let Some(text) = local_text {
            self.paste_into_input(&text);
            true
        } else if !self.yank_buffer.is_empty() {
            let text = self.yank_buffer.clone();
            self.paste_into_input(&text);
            true
        } else {
            false
        }
    }

    pub fn get_input_selection_range(&self) -> Option<(usize, usize)> {
        if let Some(start) = self.input_selection_start {
            let end = self.input_cursor_idx;
            Some((start.min(end), start.max(end)))
        } else {
            None
        }
    }

    pub fn delete_selected_input(&mut self) -> bool {
        if let Some((start, end)) = self.get_input_selection_range() {
            let chars: Vec<char> = self.input.chars().collect();
            let mut new_chars = Vec::new();
            new_chars.extend_from_slice(&chars[..start]);
            new_chars.extend_from_slice(&chars[end..]);
            self.input = new_chars.into_iter().collect();
            self.input_cursor_idx = start;
            self.input_selection_start = None;
            true
        } else {
            false
        }
    }

    /// Number of wrapped lines in the (folded) transcript, used for scroll math.
    fn wrapped_line_count(&self, width: usize) -> usize {
        if width == 0 {
            return 0;
        }
        let (folded, _) = self.fold();
        Paragraph::new(folded.as_str())
            .wrap(Wrap { trim: false })
            .line_count(width as u16) as usize
    }

    /// Build a folded copy of the transcript: every collapsed tool-card body is
    /// replaced by a single placeholder line. Returns the folded string plus the
    /// region list used to translate folded byte offsets back to `output_text`.
    fn fold(&self) -> (String, Vec<FoldRegion>) {
        let mut folded = String::with_capacity(self.output_text.len());
        let mut regions = Vec::new();
        let mut last = 0usize;
        for card in self.tool_cards.iter() {
            if !card.completed || !card.collapsed {
                continue;
            }
            if card.body_start < last || card.body_end > self.output_text.len() {
                continue;
            }
            folded.push_str(&self.output_text[last..card.body_start]);
            let fs = folded.len();
            let line_count = self.output_text[card.body_start..card.body_end]
                .lines()
                .count();
            let placeholder = format!("  ··· {line_count} lines (click header to expand)");
            folded.push_str(&placeholder);
            regions.push(FoldRegion {
                folded_start: fs,
                folded_end: fs + placeholder.len(),
                orig_start: card.body_start,
                orig_end: card.body_end,
            });
            last = card.body_end;
        }
        if last < self.output_text.len() {
            folded.push_str(&self.output_text[last..]);
        }
        (folded, regions)
    }

    /// Translate a folded byte offset back to an `output_text` byte offset.
    fn map_folded_to_orig(&self, regions: &[FoldRegion], fidx: usize) -> usize {
        let mut delta = 0isize;
        for r in regions {
            if fidx < r.folded_start {
                break;
            }
            if fidx < r.folded_end {
                return r.orig_start + (fidx - r.folded_start);
            }
            delta +=
                (r.orig_end as isize - r.orig_start as isize) - (r.folded_end as isize - r.folded_start as isize);
        }
        (fidx as isize + delta).max(0) as usize
    }

    /// If `fidx` lies inside a collapsed body's placeholder, returns the original
    /// body start (used as the selection base index for the placeholder line).
    fn folded_placeholder_orig(&self, regions: &[FoldRegion], fidx: usize) -> Option<usize> {
        for r in regions {
            if fidx >= r.folded_start && fidx < r.folded_end {
                return Some(r.orig_start);
            }
            if fidx < r.folded_start {
                break;
            }
        }
        None
    }

    /// Toggle a completed tool card when `char_idx` is inside its header. Returns
    /// true when a card was toggled (callers should skip starting a selection).
    fn toggle_tool_card_at(&mut self, char_idx: usize) -> bool {
        let target = self.tool_cards.iter().position(|c| {
            c.completed && char_idx >= c.header_start && char_idx < c.header_end
        });
        if let Some(idx) = target {
            let card = &mut self.tool_cards[idx];
            card.collapsed = !card.collapsed;
            let glyph = if card.collapsed { "▸" } else { "▾" };
            self.output_text
                .replace_range(card.header_start..card.header_start + glyph.len(), glyph);
            true
        } else {
            false
        }
    }

    fn push_history(&mut self, entry: &str) {
        if self.input_history.last().map(|l| l.as_str()) == Some(entry) {
            return;
        }
        self.input_history.push(entry.to_string());
        if self.input_history.len() > 200 {
            self.input_history.remove(0);
        }
        self.history_index = None;
    }

    fn history_back(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        self.history_index = Some(match self.history_index {
            Some(i) if i > 0 => i - 1,
            Some(_) => 0,
            None => self.input_history.len() - 1,
        });
        if let Some(i) = self.history_index {
            self.set_input_text(self.input_history[i].clone());
        }
    }

    fn history_forward(&mut self) {
        match self.history_index {
            Some(i) if i + 1 < self.input_history.len() => {
                self.history_index = Some(i + 1);
                self.set_input_text(self.input_history[i + 1].clone());
            }
            Some(_) => {
                self.history_index = None;
                self.set_input_text(String::new());
            }
            None => {}
        }
    }

    fn set_input_text(&mut self, text: String) {
        self.input = text;
        self.input_cursor_idx = self.input.chars().count();
        self.input_selection_start = None;
    }

    fn move_cursor_vertically(&mut self, delta: isize) {
        let (row, col) = self.get_input_cursor_row_col();
        let chars: Vec<char> = self.input.chars().collect();
        let mut line_starts = vec![0usize];
        for (i, c) in chars.iter().enumerate() {
            if *c == '\n' {
                line_starts.push(i + 1);
            }
        }
        let target = row as isize + delta;
        if target < 0 || target as usize >= line_starts.len() {
            return;
        }
        let start = line_starts[target as usize];
        let end = line_starts
            .get(target as usize + 1)
            .copied()
            .unwrap_or(chars.len());
        let new_col = col.min((end - start) as u16);
        self.input_cursor_idx = start + new_col as usize;
    }

    pub fn get_styled_output_lines(&self) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let (sel_start, sel_end) = match self.selection {
            Some((s, e)) => (s.min(e), s.max(e)),
            None => (0, 0),
        };

        let (folded, regions) = self.fold();
        let mut fidx = 0usize;
        let mut inside_code_block = false;

        for line_str in folded.split('\n') {
            let fls = fidx;
            let fle = fls + line_str.len();
            fidx = fle + 1;

            if let Some(orig_body_start) = self.folded_placeholder_orig(&regions, fls) {
                let line_with_sel = parse_line_markdown(
                    line_str,
                    orig_body_start,
                    sel_start,
                    sel_end,
                    false,
                    true,
                    THEME.text_muted,
                );
                lines.push(line_with_sel);
                continue;
            }

            let os = self.map_folded_to_orig(&regions, fls);
            let oe = self.map_folded_to_orig(&regions, fle);
            let orig_line = &self.output_text[os..oe];

            let trimmed = orig_line.trim();
            if trimmed.starts_with("```") {
                inside_code_block = !inside_code_block;
                let line_with_sel = parse_line_markdown(
                    orig_line,
                    os,
                    sel_start,
                    sel_end,
                    false,
                    true,
                    THEME.text_muted,
                );
                lines.push(line_with_sel);
            } else {
                let mut line_with_sel = parse_line_markdown(
                    orig_line,
                    os,
                    sel_start,
                    sel_end,
                    inside_code_block,
                    false,
                    THEME.text,
                );
                if inside_code_block {
                    add_line_bg(&mut line_with_sel, THEME.code_bg);
                }
                lines.push(line_with_sel);
            }
        }
        lines
    }

    pub fn get_styled_input_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let (sel_start, sel_end) = match self.get_input_selection_range() {
            Some((s, e)) => (s, e),
            None => (0, 0),
        };
        
        let text = &self.input;
        let mut current_line_spans = Vec::new();
        let chars: Vec<char> = text.chars().collect();
        
        for (idx, &c) in chars.iter().enumerate() {
            if c == '\n' {
                lines.push(Line::from(current_line_spans));
                current_line_spans = Vec::new();
            } else {
                let is_selected = idx >= sel_start && idx < sel_end;
                let style = if is_selected {
                    Style::default().bg(THEME.selection_bg).fg(THEME.text)
                } else {
                    Style::default().fg(THEME.text)
                };
                current_line_spans.push(Span::styled(c.to_string(), style));
            }
        }
        lines.push(Line::from(current_line_spans));
        
        lines
    }

    pub fn get_input_cursor_row_col(&self) -> (u16, u16) {
        let chars: Vec<char> = self.input.chars().collect();
        let mut row = 0;
        let mut col = 0;
        for (idx, &c) in chars.iter().enumerate() {
            if idx >= self.input_cursor_idx {
                break;
            }
            if c == '\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (row, col)
    }

    pub fn map_coordinates_to_char_idx(&self, x: u16, y: u16) -> Option<usize> {
        let area = self.last_output_area.get();
        if area.width <= 2 || area.height <= 2 {
            return None;
        }
        let inner_x = area.x + 1;
        let inner_y = area.y + 1;
        let inner_w = area.width - 2;
        let inner_h = area.height - 2;
        
        if x < inner_x || x >= inner_x + inner_w || y < inner_y || y >= inner_y + inner_h {
            return None;
        }
        
        let scroll_y = self.output_scroll_y(area.width, area.height);
        let wrapped_line_idx = (y - inner_y) + scroll_y;
        
        let (folded, regions) = self.fold();
        let wrapped_lines = wrap_text(&folded, inner_w as usize);
        if (wrapped_line_idx as usize) < wrapped_lines.len() {
            let range = &wrapped_lines[wrapped_line_idx as usize];
            let col = (x - inner_x) as usize;
            
            let line_sub = &folded[range.start..range.end];
            let mut byte_offset = 0;
            for (char_idx, (b_idx, _)) in line_sub.char_indices().enumerate() {
                if char_idx == col {
                    byte_offset = b_idx;
                    break;
                }
                byte_offset = b_idx + 1;
            }
            let orig_start = self.map_folded_to_orig(&regions, range.start);
            Some(orig_start + byte_offset)
        } else {
            None
        }
    }

    pub fn select_word_at(&mut self, char_idx: usize) {
        let text = &self.output_text;
        if char_idx >= text.len() {
            return;
        }
        
        let mut word_start = char_idx;
        while word_start > 0 {
            let prev_char = text[..word_start].chars().next_back();
            if let Some(c) = prev_char {
                if c.is_alphanumeric() || c == '_' {
                    word_start -= c.len_utf8();
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        
        let mut word_end = char_idx;
        while word_end < text.len() {
            let next_char = text[word_end..].chars().next();
            if let Some(c) = next_char {
                if c.is_alphanumeric() || c == '_' {
                    word_end += c.len_utf8();
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        
        self.selection = Some((word_start, word_end));
    }

    pub fn select_line_at(&mut self, char_idx: usize) {
        let text = &self.output_text;
        if char_idx >= text.len() {
            return;
        }
        
        let mut line_start = char_idx;
        while line_start > 0 {
            let prev_char = text[..line_start].chars().next_back();
            if let Some(c) = prev_char {
                if c == '\n' {
                    break;
                }
                line_start -= c.len_utf8();
            } else {
                break;
            }
        }
        
        let mut line_end = char_idx;
        while line_end < text.len() {
            let next_char = text[line_end..].chars().next();
            if let Some(c) = next_char {
                if c == '\n' {
                    break;
                }
                line_end += c.len_utf8();
            } else {
                break;
            }
        }
        
        self.selection = Some((line_start, line_end));
    }

    fn current_model_index(&self) -> usize {
        self.available_models
            .iter()
            .position(|m| m == &self.model)
            .unwrap_or(self.model_picker_index)
    }

    /// Scroll offset so the newest output stays visible (accounting for borders + wrap).
    fn output_scroll_y(&self, area_width: u16, area_height: u16) -> u16 {
        let inner_width = area_width.saturating_sub(2);
        let inner_height = area_height.saturating_sub(2);
        if inner_width == 0 || inner_height == 0 {
            return 0;
        }
        // line_count is usize; ratatui scroll is u16 — clamp to avoid wrap on long sessions.
        let total_lines = self
            .wrapped_line_count(inner_width as usize)
            .min(u16::MAX as usize) as u16;
        total_lines.saturating_sub(inner_height)
    }

    pub fn open_model_picker(&mut self) {
        if self.available_models.len() <= 1 {
            return;
        }
        self.model_picker_index = self.current_model_index();
        self.show_model_picker = true;
        self.show_help = false;
    }

    pub fn close_model_picker(&mut self) {
        self.show_model_picker = false;
    }

    pub fn cycle_model(&mut self, delta: isize) -> Option<String> {
        if self.available_models.is_empty() {
            return None;
        }
        if !self.show_model_picker {
            self.open_model_picker();
        }
        let len = self.available_models.len() as isize;
        let next = (self.model_picker_index as isize + delta).rem_euclid(len) as usize;
        self.model_picker_index = next;
        Some(self.available_models[next].clone())
    }

    /// Cycle permission mode: ask → allow → plan → ask.
    pub fn cycle_permission(&mut self, agent: &Agent) {
        let next = match self.permission_mode.as_str() {
            "ask" => "allow",
            "allow" => "plan",
            _ => "ask",
        };
        agent.set_permission_mode(next);
        self.permission_mode = next.to_string();
        self.status = format!("permission: {next}");
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(5),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area);

        // Middle row: output transcript | todo side panel.
        let side_w = ((chunks[1].width * self.todo_panel_pct) / 100).clamp(18, 44);
        let (output_area, side_area) = if self.show_todo_panel && chunks[1].width > side_w + 24 {
            let split = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(20), Constraint::Length(side_w)])
                .split(chunks[1]);
            (split[0], split[1])
        } else {
            (chunks[1], Rect::default())
        };
        self.side_area.set(side_area);
        self.last_output_area.set(output_area);

        let status_style = if self.status == "idle" {
            Style::default().fg(THEME.success)
        } else {
            Style::default().fg(THEME.warning).add_modifier(Modifier::BOLD)
        };
        let spinner = if self.streaming {
            let frame_idx = (self.frame % SPINNER_FRAMES.len() as u64) as usize;
            format!("{} ", SPINNER_FRAMES[frame_idx])
        } else {
            String::new()
        };
        let header = Paragraph::new(Line::from(vec![
            Span::styled(" Forge ", Style::default().fg(THEME.text_accent).add_modifier(Modifier::BOLD)),
            Span::raw(" | "),
            Span::styled(
                format!("{} · {}", self.provider, self.model),
                Style::default().fg(THEME.warning),
            ),
            Span::raw(" | "),
            Span::styled(
                format!("session: {}", &self.session_id[..8.min(self.session_id.len())]),
                Style::default().fg(THEME.text_muted),
            ),
            Span::raw(" | "),
            Span::styled(format!("{spinner}{}", self.status), status_style),
        ]));
        frame.render_widget(header, chunks[0]);

        let scroll_y = self.output_scroll_y(output_area.width, output_area.height);
        let output = Paragraph::new(self.get_styled_output_lines())
            .block(Block::default().borders(Borders::ALL).title(" Output "))
            .wrap(Wrap { trim: false })
            .scroll((scroll_y, 0));
        frame.render_widget(output, output_area);

        if side_area.width > 0 {
            self.render_todo_panel(frame, side_area);
        }

        let (input_row, input_col) = self.get_input_cursor_row_col();
        let input_inner_h = chunks[2].height.saturating_sub(2);
        let input_scroll_y = input_row.saturating_sub(input_inner_h.saturating_sub(1));
        
        let input_inner_w = chunks[2].width.saturating_sub(2);
        let input_scroll_x = input_col.saturating_sub(input_inner_w.saturating_sub(1));

        let input_block = |title: &'static str| {
            Block::default().borders(Borders::ALL).title(title)
        };
        let input = if let Some(pa) = &self.pending_approval {
            let is_plan = self.permission_mode == "plan";
            let title = if is_plan {
                " Plan — approve proposal? "
            } else {
                " Approval Required "
            };
            let verb = if is_plan { "Propose" } else { "Allow" };
            let hint = if is_plan {
                "  [y execute · n/Esc skip]"
            } else {
                "  [y allow · n/Esc deny]"
            };
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("⚠ {verb} "),
                    Style::default().fg(THEME.warning).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{}({})", pa.tool_name, pa.arguments),
                    Style::default().fg(THEME.text),
                ),
                Span::styled(hint, Style::default().fg(THEME.text_muted)),
            ]))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(THEME.warning)),
            )
        } else if self.input.is_empty() {
            Paragraph::new(Line::from(vec![Span::styled(
                "Type a message… (Enter send · Shift+Enter newline · ↑/↓ history)",
                Style::default().fg(THEME.text_muted),
            )]))
            .block(input_block(" Input "))
        } else {
            Paragraph::new(self.get_styled_input_lines())
                .block(input_block(" Input "))
                .scroll((input_scroll_y, input_scroll_x))
        };
        frame.render_widget(input, chunks[2]);

        let footer = Paragraph::new(self.status_bar(chunks[3].width));
        frame.render_widget(footer, chunks[3]);

        if self.show_help {
            self.render_help(frame, area);
        }

        if self.show_model_picker {
            self.render_model_picker(frame, area);
        }

        if self.pending_options.is_some() {
            self.render_options_picker(frame, area);
        }

        // Place blinking terminal cursor at the scroll-adjusted cursor coordinate
        if self.pending_approval.is_none() && self.pending_options.is_none() {
            let cursor_x = chunks[2].x + 1 + input_col.saturating_sub(input_scroll_x);
            let cursor_y = chunks[2].y + 1 + input_row.saturating_sub(input_scroll_y);
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }

    fn render_todo_panel(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .todos
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let marker = if t.done { "[x]" } else { "[ ]" };
                let text = format!("{marker} {}", truncate_str(&t.text, area.width.saturating_sub(3) as usize));
                let base = if t.done {
                    Style::default().fg(THEME.text_muted)
                } else {
                    Style::default().fg(THEME.text)
                };
                if self.todo_focus && i == self.todo_selected {
                    ListItem::new(text).style(base.bg(THEME.surface_alt))
                } else {
                    ListItem::new(text).style(base)
                }
            })
            .collect();
        let border = if self.todo_focus {
            Style::default().fg(THEME.text_accent)
        } else {
            Style::default().fg(THEME.text_muted)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" Todos ({}) ", self.todos.len()))
            .border_style(border);
        let content = if items.is_empty() {
            vec![ListItem::new(Line::from(vec![Span::styled(
                "(empty — tell the agent to add tasks)",
                Style::default().fg(THEME.text_muted),
            )]))]
        } else {
            items
        };
        frame.render_widget(List::new(content).block(block), area);
    }

    fn render_options_picker(&self, frame: &mut Frame, area: Rect) {
        let Some(o) = &self.pending_options else { return };
        let row_count = o.options.len() + 1;
        let w = 72.min(area.width.saturating_sub(4));
        let h = (row_count as u16 + 5).min(area.height.saturating_sub(4));
        let inner = centered_rect_px(w, h, area);
        self.options_area.set(inner);

        let mut items: Vec<ListItem> = Vec::new();
        for (i, opt) in o.options.iter().enumerate() {
            let selected = i == o.selected;
            let marker = if selected { "> " } else { "  " };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{marker}{}. ", i + 1),
                    Style::default().fg(if selected { THEME.on_accent } else { THEME.text_muted }),
                ),
                Span::styled(
                    opt.clone(),
                    if selected {
                        Style::default().fg(THEME.on_accent).bg(THEME.text_accent)
                    } else {
                        Style::default().fg(THEME.text)
                    },
                ),
            ])));
        }
        let custom_selected = o.selected >= o.options.len();
        let custom_display = if custom_selected {
            o.custom.clone()
        } else {
            "(click to type)".into()
        };
        items.push(ListItem::new(Line::from(vec![
            Span::styled(
                format!(
                    "{}Type your own answer: ",
                    if custom_selected { "> " } else { "  " }
                ),
                Style::default().fg(if custom_selected { THEME.on_accent } else { THEME.text_muted }),
            ),
            Span::styled(
                custom_display,
                if custom_selected {
                    Style::default().fg(THEME.on_accent).bg(THEME.text_accent)
                } else {
                    Style::default().fg(THEME.text_accent)
                },
            ),
        ])));

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", truncate_str(&o.prompt, 60)))
                .border_style(Style::default().fg(THEME.text_accent)),
        );
        frame.render_widget(list, inner);
    }

    fn status_bar(&self, width: u16) -> Line<'static> {
        let key_hint = if self.pending_approval.is_some() {
            if self.permission_mode == "plan" {
                " y = execute plan · n/Esc = skip "
            } else {
                " y = allow · n/Esc = deny "
            }
        } else if self.pending_options.is_some() {
            " ↑/↓ choose · Enter pick · Esc dismiss "
        } else if self.todo_focus {
            " ↑/↓ move · Space toggle · d delete · [ ] resize · Tab/Esc back to input "
        } else if self.streaming {
            " Esc / Ctrl+C: stop · mouse wheel: scroll · click tool header: expand/collapse "
        } else {
            " Enter: send · Shift+Enter: newline · ↑/↓: history · Tab: model · Shift+Tab: perm · Ctrl+T: todos · /help "
        };

        let total = format_number(self.tokens_total);
        let pct = if self.context_window > 0 {
            (self.tokens_total as usize * 100) / self.context_window
        } else {
            0
        };
        let mut info = format!("{total} tokens · {pct}% used · perm: {}", self.permission_mode);
        if self.total_cost > 0.0 {
            info.push_str(&format!(" · ${:.2}", self.total_cost));
        }

        let pad = width.saturating_sub(key_hint.len() as u16).saturating_sub(info.len() as u16 + 2);
        let mut spans = vec![Span::styled(key_hint, Style::default().fg(THEME.text_muted))];
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad as usize), Style::default()));
        }
        spans.push(Span::styled(
            info,
            Style::default().fg(if self.tokens_total > 0 { THEME.success } else { THEME.text_muted }),
        ));
        Line::from(spans)
    }

    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let help_area = centered_rect(60, 70, area);
        let help_text = vec![
            "/help      - Show this help",
            "/model     - Show or set model (/model gpt-4o)",
            "/tools     - List available tools",
            "/new       - Start a fresh session",
            "/compact   - Summarize older messages",
            "/clear     - Clear output",
            "/debug     - Toggle debug mode",
            "/parallel  - Run parallel tasks (/parallel task1; task2)",
            "/ssh       - SSH commands (/ssh list, /ssh connect <alias>)",
            "/todo      - Toggle the todo panel (/todo add <task>, /todo clear)",
            "/resume    - Resume last session",
            "/skills    - List loaded skills",
            "/quit      - Exit",
            "/exit      - Exit",
        ];
        let items: Vec<ListItem> = help_text.iter().map(|s| ListItem::new(*s)).collect();
        let help = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Slash Commands ")
                .style(Style::default().bg(THEME.surface)),
        );
        frame.render_widget(help, help_area);
    }

    fn render_model_picker(&self, frame: &mut Frame, area: Rect) {
        let picker_area = centered_rect(50, 40, area);
        let items: Vec<ListItem> = self
            .available_models
            .iter()
            .enumerate()
            .map(|(idx, model)| {
                let prefix = if idx == self.model_picker_index {
                    "▸ "
                } else {
                    "  "
                };
                let style = if idx == self.model_picker_index {
                    Style::default()
                        .fg(THEME.warning)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(THEME.text)
                };
                ListItem::new(format!("{prefix}{model}")).style(style)
            })
            .collect();
        let picker = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Select Model (Tab/Shift+Tab, Enter/Esc) ")
                .style(Style::default().bg(THEME.surface)),
        );
        frame.render_widget(picker, picker_area);
    }

    fn toggle_todo_panel(&mut self) {
        self.show_todo_panel = !self.show_todo_panel;
        if !self.show_todo_panel {
            self.todo_focus = false;
        }
    }

    fn resize_todo_panel(&mut self, delta: i16) {
        self.todo_panel_pct =
            (self.todo_panel_pct as i16 + delta).clamp(15, 55) as u16;
    }

    fn toggle_todo_at(&mut self, idx: usize) {
        if let Some(item) = self.todos.get_mut(idx) {
            item.done = !item.done;
        }
    }

    fn todo_add(&mut self, text: &str) {
        self.todos.push(TodoItem::new(text.trim().to_string()));
        self.todo_selected = self.todos.len().saturating_sub(1);
    }

    fn todo_remove_at(&mut self, idx: usize) {
        if idx < self.todos.len() {
            self.todos.remove(idx);
            self.todo_selected = self.todo_selected.min(self.todos.len().saturating_sub(1));
        }
    }

    /// Push the buffered thought stream into the transcript as a timed block.
    fn end_thought_block(&mut self) {
        let duration = self
            .thinking_start
            .take()
            .map(|t| {
                let d = t.elapsed().as_secs_f64();
                format!("{d:.1}s")
            })
            .unwrap_or_else(|| "…".into());
        let thought = std::mem::take(&mut self.current_thought);
        self.end_stream();
        if thought.trim().is_empty() {
            return;
        }
        self.ensure_newline();
        let header = format!("💭 Thought: {duration}");
        self.output_text.push_str(&header);
        self.output_text.push('\n');
        let body = truncate_str(thought.trim(), 1200);
        for line in body.lines() {
            self.output_text.push_str("  ");
            self.output_text.push_str(line);
            self.output_text.push('\n');
        }
        self.output_text.push('\n');
        self.normalize_trailing_newlines();
        self.trim_output_if_needed();
    }
}

fn centered_rect_px(width: u16, height: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(r.height.saturating_sub(height) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(r.width.saturating_sub(width) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(vertical[1])[1]
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn set_model(
    agent: &std::sync::Arc<Agent>,
    session: &mut Session,
    app: &mut TuiApp,
    model: String,
) {
    agent.set_model(model.clone());
    session.model = model.clone();
    app.model = model;
    app.provider = agent.provider_name();
    app.model_price = agent.pricing();
    if let Some(idx) = app.available_models.iter().position(|m| m == &app.model) {
        app.model_picker_index = idx;
    }
}

enum CommandOutcome {
    Continue,
    Quit,
}

fn apply_agent_event(app: &mut TuiApp, event: AgentEvent) -> bool {
    match event {
        AgentEvent::Thinking => {
            app.end_stream();
            app.current_thought.clear();
            app.thinking_start = Some(Instant::now());
            app.status = "thinking...".into();
        }
        AgentEvent::ThinkingDelta { text } => {
            app.current_thought.push_str(&text);
        }
        AgentEvent::ThinkingEnd => {
            app.end_thought_block();
        }
        AgentEvent::ContentDelta { text } => {
            app.append_stream(&text, Style::default().fg(THEME.text));
        }
        AgentEvent::ToolCallStart { name, arguments } => {
            app.end_stream();
            let args_short = truncate_str(&arguments, 120);
            let header = format!("▾ {name} {args_short}");
            app.ensure_newline();
            let header_start = app.output_text.len();
            app.output_text.push_str(&header);
            app.output_text.push('\n');
            let header_end = header_start + header.len();
            let body_start = header_end + 1;
            let card_idx = app.tool_cards.len();
            app.tool_cards.push(ToolCard {
                header_start,
                header_end,
                body_start,
                body_end: body_start,
                collapsed: false,
                is_error: false,
                completed: false,
            });
            app.pending_tool_cards.push_back(card_idx);
        }
        AgentEvent::ToolCallEnd { name, output, is_error } => {
            app.end_stream();
            if let Some(card_idx) = app.pending_tool_cards.pop_front() {
                let body = format_tool_body(&name, &output, is_error);
                app.output_text.push_str(&body);
                if let Some(card) = app.tool_cards.get_mut(card_idx) {
                    card.completed = true;
                    card.is_error = is_error;
                    card.body_end = app.output_text.len();
                    let line_count =
                        app.output_text[card.body_start..card.body_end].lines().count();
                    if line_count > TOOL_CARD_COLLAPSE_THRESHOLD {
                        card.collapsed = true;
                        let glyph = "▸";
                        app.output_text.replace_range(
                            card.header_start..card.header_start + glyph.len(),
                            glyph,
                        );
                    }
                }
            }
            app.trim_output_if_needed();
        }
        AgentEvent::ApprovalRequest { tool_name, arguments } => {
            app.end_stream();
            let args_short = truncate_str(&arguments, 120);
            let is_plan = app.permission_mode == "plan";
            app.push_output(
                &format!(
                    "⚠ {tool_name} {}: {args_short}",
                    if is_plan {
                        "proposal"
                    } else {
                        "requires confirmation"
                    }
                ),
                Style::default().fg(THEME.warning),
            );
            app.status = if is_plan {
                "plan proposal awaiting approval (y/N)".into()
            } else {
                "awaiting approval (y/N)".into()
            };
            app.pending_approval = Some(PendingApproval {
                tool_name,
                arguments: args_short,
            });
        }
        AgentEvent::OptionsRequest { prompt, options } => {
            app.end_stream();
            app.pending_options = Some(OptionsState {
                prompt,
                options,
                selected: 0,
                custom: String::new(),
            });
            app.status = "choose an option".into();
        }
        AgentEvent::TodoUpdate { items } => {
            app.todos = items;
            app.todo_selected = app.todo_selected.min(app.todos.len().saturating_sub(1));
        }
        AgentEvent::Retrying { attempt, error } => {
            app.end_stream();
            app.push_output(
                &format!("↻ retry {attempt}: {error}"),
                Style::default().fg(THEME.text_muted),
            );
        }
        AgentEvent::Error { message } => {
            app.end_stream();
            app.push_output(
                &format!("[error] {message}"),
                Style::default().fg(THEME.error),
            );
        }
        AgentEvent::TokenUsage { prompt, completion, total } => {
            app.last_usage = Some((prompt, completion, total));
            app.tokens_total = total as u64;
            if let Some((in_price, out_price)) = app.model_price {
                let cost = (prompt as f64 * in_price + completion as f64 * out_price) / 1_000_000.0;
                app.total_cost += cost;
            }
            app.status = format!("done ({total} tokens)");
        }
        AgentEvent::Done => {
            app.end_stream();
            if app.thinking_start.is_some() || !app.current_thought.is_empty() {
                app.end_thought_block();
            }
            app.ensure_newline();
            app.output_text.push('\n');
            return true;
        }
        _ => {}
    }
    false
}

/// Render the body of a completed tool card: a status line plus indented result.
fn format_tool_body(name: &str, output: &str, is_error: bool) -> String {
    let output = truncate_str(output, 4000);
    let line_count = output.lines().count();
    let mut body = String::new();
    if is_error {
        body.push_str(&format!("  ✖ {name} failed: {output}\n"));
    } else if line_count > 1 {
        body.push_str(&format!("  ✔ {name} completed ({line_count} lines of output)\n"));
        for line in output.lines() {
            body.push_str("  ");
            body.push_str(line);
            body.push('\n');
        }
    } else {
        body.push_str(&format!("  ✔ {name} completed: {output}\n"));
    }
    body
}

async fn finish_inflight(
    app: &mut TuiApp,
    session: &mut Session,
    store: &SessionStore,
    mut in_flight: InFlight,
    abort: bool,
    channels: UiChannels<'_>,
) {
    *channels.approval = None;
    *channels.options = None;
    app.pending_approval = None;
    app.pending_options = None;
    app.streaming = false;
    session.todos = app.todos.clone();

    while let Ok(event) = in_flight.rx.try_recv() {
        apply_agent_event(app, event);
    }

    if abort && !in_flight.handle.is_finished() {
        in_flight.handle.abort();
        app.push_output(
            "[cancelled] agent turn aborted",
            Style::default().fg(THEME.warning),
        );
        app.status = "idle".into();
        app.end_stream();
        return;
    }

    // Never block the UI forever if the agent task stalls after Done.
    let join_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        &mut in_flight.handle,
    )
    .await;

    match join_result {
        Ok(Ok((updated_session, result))) => {
            while let Ok(event) = in_flight.rx.try_recv() {
                apply_agent_event(app, event);
            }
            *session = updated_session;
            if let Err(e) = result {
                app.push_output(&format!("Error: {e}"), Style::default().fg(THEME.error));
            }
            let _ = store.save(session);
        }
        Ok(Err(e)) => {
            app.push_output(
                &format!("Agent task failed: {e}"),
                Style::default().fg(THEME.error),
            );
        }
        Err(_) => {
            in_flight.handle.abort();
            app.push_output(
                "[timeout] agent task did not finish; aborted so UI can accept input",
                Style::default().fg(THEME.warning),
            );
        }
    }
    app.status = "idle".into();
    app.end_stream();
}

pub async fn run_tui(
    agent: std::sync::Arc<Agent>,
    session: &mut Session,
    store: &SessionStore,
    ssh_manager: Option<std::sync::Arc<forge_ssh::SshManager>>,
    workspace: std::path::PathBuf,
    skill_loader: std::sync::Arc<forge_tool::SkillLoader>,
    available_models: Vec<String>,
) -> Result<()> {
    use crossterm::{
        event::{
            DisableBracketedPaste, EnableBracketedPaste,
            Event, EventStream, KeyCode, KeyEventKind, KeyModifiers,
        },
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    };
    use futures::StreamExt;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;
    // Full clear so we start with a known terminal state.
    terminal.clear()?;

    let mut app = TuiApp::new(session, available_models);
    app.provider = agent.provider_name();
    app.context_window = agent.context_window();
    app.permission_mode = agent.permission_mode();
    app.model_price = agent.pricing();
    app.total_cost = agent.total_cost();
    let mut in_flight: Option<InFlight> = None;
    // UI -> agent channel for interactive tool approvals (recreated per turn).
    let mut approval_resp_tx: Option<UnboundedSender<bool>> = None;
    // UI -> agent channel for options-picker responses (recreated per turn).
    let mut options_resp_tx: Option<UnboundedSender<String>> = None;
    // Async event stream — never block a tokio worker with event::poll.
    // Blocking poll was starving the agent task after the first response on
    // single-worker / contended runtimes, which made the UI appear hung.
    let mut events = EventStream::new();
    let mut should_quit = false;

    let mut current_mouse_state = app.mouse_enabled;
    if current_mouse_state {
        execute!(io::stdout(), crossterm::event::EnableMouseCapture)?;
    }

    while !should_quit {
        if app.mouse_enabled != current_mouse_state {
            if app.mouse_enabled {
                execute!(io::stdout(), crossterm::event::EnableMouseCapture)?;
            } else {
                execute!(io::stdout(), crossterm::event::DisableMouseCapture)?;
            }
            current_mouse_state = app.mouse_enabled;
        }
        terminal.draw(|f| app.render(f))?;

        if let Some(ref mut running) = in_flight {
            let mut done = false;
            while let Ok(event) = running.rx.try_recv() {
                if apply_agent_event(&mut app, event) {
                    done = true;
                }
            }
            if done || running.handle.is_finished() {
                let finished = in_flight.take().expect("in_flight");
                finish_inflight(
                    &mut app,
                    session,
                    store,
                    finished,
                    false,
                    UiChannels {
                        approval: &mut approval_resp_tx,
                        options: &mut options_resp_tx,
                    },
                )
                .await;
                // Resync ratatui's diff buffer if anything wrote to the tty mid-turn.
                let _ = terminal.clear();
                continue;
            }
        }

                tokio::select! {
            maybe = events.next() => {
                match maybe {
                    Some(Ok(event)) => {
                        if let Event::Paste(text) = &event {
                            app.paste_into_input(text);
                            continue;
                        }

                        if let Event::Mouse(mouse_event) = event {
                            if app.mouse_enabled {
                                match mouse_event.kind {
                                    crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                                        // Options picker: click an option row to select it.
                                        if app.pending_options.is_some() {
                                            let oarea = app.options_area.get();
                                            let row_count = app
                                                .pending_options
                                                .as_ref()
                                                .map(|o| o.options.len() + 1)
                                                .unwrap_or(0);
                                            if oarea.width > 0
                                                && mouse_event.column >= oarea.x
                                                && mouse_event.column < oarea.x + oarea.width
                                                && mouse_event.row > oarea.y
                                                && mouse_event.row < oarea.y + 1 + row_count as u16
                                            {
                                                let idx = (mouse_event.row - (oarea.y + 1)) as usize;
                                                if let Some(o) = app.pending_options.as_mut() {
                                                    if idx <= o.options.len() {
                                                        o.selected = idx;
                                                    }
                                                }
                                                continue;
                                            }
                                        }
                                        // Todo side panel: click a row toggles it.
                                        if app.show_todo_panel && !app.todos.is_empty() {
                                            let sarea = app.side_area.get();
                                            if mouse_event.column >= sarea.x
                                                && mouse_event.column < sarea.x + sarea.width
                                                && mouse_event.row > sarea.y
                                                && mouse_event.row < sarea.y + 1 + app.todos.len() as u16
                                            {
                                                let idx = (mouse_event.row - (sarea.y + 1)) as usize;
                                                if idx < app.todos.len() {
                                                    app.toggle_todo_at(idx);
                                                    app.todo_selected = idx;
                                                    app.todo_focus = true;
                                                    session.todos = app.todos.clone();
                                                    let _ = store.save(session);
                                                }
                                                continue;
                                            }
                                        }
                                        if let Some(char_idx) = app.map_coordinates_to_char_idx(mouse_event.column, mouse_event.row) {
                                            // Clicking a tool-card header toggles expand/collapse instead of selecting.
                                            if app.toggle_tool_card_at(char_idx) {
                                                continue;
                                            }
                                            app.is_selecting_mouse = true;
                                            app.selection = Some((char_idx, char_idx));
                                            
                                            let now = Instant::now();
                                            let is_multi_click = if let Some((last_time, last_char_idx)) = app.double_click_state {
                                                now.duration_since(last_time).as_millis() < 400 && last_char_idx == char_idx
                                            } else {
                                                false
                                            };
                                            
                                            if is_multi_click {
                                                match app.last_click_type {
                                                    ClickType::Single => {
                                                        app.last_click_type = ClickType::Double;
                                                        app.select_word_at(char_idx);
                                                    }
                                                    ClickType::Double => {
                                                        app.last_click_type = ClickType::Triple;
                                                        app.select_line_at(char_idx);
                                                    }
                                                    ClickType::Triple => {
                                                        app.last_click_type = ClickType::Single;
                                                        app.selection = Some((char_idx, char_idx));
                                                    }
                                                }
                                            } else {
                                                app.last_click_type = ClickType::Single;
                                                app.selection = Some((char_idx, char_idx));
                                            }
                                            app.double_click_state = Some((now, char_idx));
                                        }
                                    }
                                    crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                                        if app.is_selecting_mouse {
                                            if let Some(char_idx) = app.map_coordinates_to_char_idx(mouse_event.column, mouse_event.row) {
                                                if let Some((start, _)) = app.selection {
                                                    app.selection = Some((start, char_idx));
                                                }
                                            }
                                        }
                                    }
                                    crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
                                        if app.is_selecting_mouse {
                                            app.is_selecting_mouse = false;
                                            if let Some(char_idx) = app.map_coordinates_to_char_idx(mouse_event.column, mouse_event.row) {
                                                if let Some((start, _)) = app.selection {
                                                    app.selection = Some((start, char_idx));
                                                }
                                            }
                                            app.copy_selection();
                                        }
                                    }
                                    crossterm::event::MouseEventKind::ScrollUp => {
                                        let scroll_y = app.output_scroll_y(app.last_output_area.get().width, app.last_output_area.get().height);
                                        app.scroll_offset = Some(scroll_y.saturating_sub(2));
                                    }
                                    crossterm::event::MouseEventKind::ScrollDown => {
                                        let area = app.last_output_area.get();
                                        let scroll_y = app.output_scroll_y(area.width, area.height);
                                        let inner_width = area.width.saturating_sub(2);
                                        let inner_height = area.height.saturating_sub(2);
                                        let total_lines = app
                                            .wrapped_line_count(inner_width as usize)
                                            .min(u16::MAX as usize) as u16;
                                        let max_scroll = total_lines.saturating_sub(inner_height);
                                        let next_scroll = scroll_y + 2;
                                        if next_scroll >= max_scroll {
                                            app.scroll_offset = None;
                                        } else {
                                            app.scroll_offset = Some(next_scroll);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            continue;
                        }

                        let Event::Key(key) = event else {
                            continue;
                        };
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        if app.show_model_picker {
                            match key.code {
                                KeyCode::Tab => {
                                    let delta = if key.modifiers.contains(KeyModifiers::SHIFT) {
                                        -1
                                    } else {
                                        1
                                    };
                                    if let Some(model) = app.cycle_model(delta) {
                                        set_model(&agent, session, &mut app, model);
                                    }
                                    continue;
                                }
                                KeyCode::Enter | KeyCode::Esc => {
                                    app.close_model_picker();
                                    continue;
                                }
                                _ => continue,
                            }
                        }

                        // While a tool approval is pending, the input box becomes a
                        // y/n prompt; all other keys are ignored until resolved.
                        if app.pending_approval.is_some() {
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Enter => {
                                    if let Some(tx) = &approval_resp_tx {
                                        let _ = tx.send(true);
                                    }
                                    app.pending_approval = None;
                                    app.status = "thinking...".into();
                                }
                                KeyCode::Char('n') | KeyCode::Esc => {
                                    if let Some(tx) = &approval_resp_tx {
                                        let _ = tx.send(false);
                                    }
                                    app.pending_approval = None;
                                    app.status = "thinking...".into();
                                }
                                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    if let Some(tx) = &approval_resp_tx {
                                        let _ = tx.send(false);
                                    }
                                    app.pending_approval = None;
                                    app.status = "thinking...".into();
                                }
                                _ => {}
                            }
                            continue;
                        }

                        // While an options prompt is pending, the picker overlay
                        // owns the keyboard: ↑/↓ select, Enter confirms, typing a
                        // printable char switches to a custom answer, Esc dismisses.
                        if app.pending_options.is_some() {
                            match key.code {
                                KeyCode::Up => {
                                    if let Some(o) = app.pending_options.as_mut() {
                                        if o.selected > 0 {
                                            o.selected -= 1;
                                        }
                                    }
                                }
                                KeyCode::Down => {
                                    if let Some(o) = app.pending_options.as_mut() {
                                        if o.selected < o.options.len() {
                                            o.selected += 1;
                                        }
                                    }
                                }
                                KeyCode::Home => {
                                    if let Some(o) = app.pending_options.as_mut() {
                                        o.selected = 0;
                                    }
                                }
                                KeyCode::End => {
                                    if let Some(o) = app.pending_options.as_mut() {
                                        o.selected = o.options.len();
                                    }
                                }
                                KeyCode::Enter => {
                                    let answer = app.pending_options.as_ref().map(|o| o.text());
                                    if let Some(o) = app.pending_options.take() {
                                        if let Some(tx) = &options_resp_tx {
                                            let _ = tx.send(answer.unwrap_or(o.text()));
                                        }
                                        app.status = "thinking...".into();
                                    }
                                }
                                KeyCode::Esc => {
                                    app.pending_options = None;
                                    options_resp_tx = None;
                                    app.status = "thinking...".into();
                                }
                                KeyCode::Backspace => {
                                    if let Some(o) = app.pending_options.as_mut() {
                                        o.custom.pop();
                                        o.selected = o.options.len();
                                    }
                                }
                                KeyCode::Char(c) => {
                                    if let Some(o) = app.pending_options.as_mut() {
                                        o.custom.push(c);
                                        o.selected = o.options.len();
                                    }
                                }
                                _ => {}
                            }
                            continue;
                        }

                        match key.code {
                            // Todo panel navigation (panel has focus).
                            KeyCode::Up if app.todo_focus => {
                                if app.todo_selected > 0 {
                                    app.todo_selected -= 1;
                                }
                            }
                            KeyCode::Down if app.todo_focus => {
                                if app.todo_selected + 1 < app.todos.len() {
                                    app.todo_selected += 1;
                                }
                            }
                            KeyCode::Char(' ') | KeyCode::Enter if app.todo_focus => {
                                app.toggle_todo_at(app.todo_selected);
                                session.todos = app.todos.clone();
                                let _ = store.save(session);
                            }
                            KeyCode::Char('d') | KeyCode::Delete if app.todo_focus => {
                                app.todo_remove_at(app.todo_selected);
                                session.todos = app.todos.clone();
                                let _ = store.save(session);
                            }
                            KeyCode::Tab | KeyCode::Esc if app.todo_focus => {
                                app.todo_focus = false;
                            }
                            KeyCode::Char('t')
                                if key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                if app.show_todo_panel && app.todo_focus {
                                    app.toggle_todo_panel();
                                } else if app.show_todo_panel {
                                    app.todo_focus = true;
                                } else {
                                    app.show_todo_panel = true;
                                    app.todo_focus = true;
                                }
                            }
                            KeyCode::Char(']') if app.todo_focus => {
                                app.resize_todo_panel(5);
                            }
                            KeyCode::Char('[') if app.todo_focus => {
                                app.resize_todo_panel(-5);
                            }
                            KeyCode::Char('c')
                                if key.modifiers.contains(KeyModifiers::CONTROL)
                                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
                            {
                                app.copy_output();
                            }
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                if app.get_input_selection_range().is_some() {
                                    if let Some((s, e)) = app.get_input_selection_range() {
                                        let chars: Vec<char> = app.input.chars().collect();
                                        let selected_text: String = chars[s..e].iter().collect();
                                        write_clipboard(&selected_text, &mut app);
                                        app.status = "copied input selection".into();
                                    }
                                }
                                else if app.selection.is_some() {
                                    app.copy_selection();
                                }
                                else if in_flight.is_some() {
                                    if let Some(running) = in_flight.take() {
                                        finish_inflight(
                                            &mut app,
                                            session,
                                            store,
                                            running,
                                            true,
                                            UiChannels {
                                                approval: &mut approval_resp_tx,
                                                options: &mut options_resp_tx,
                                            },
                                        )
                                        .await;
                                    }
                                }
                                else {
                                    should_quit = true;
                                }
                            }
                            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                if let Some((s, e)) = app.get_input_selection_range() {
                                    let chars: Vec<char> = app.input.chars().collect();
                                    let selected_text: String = chars[s..e].iter().collect();
                                    write_clipboard(&selected_text, &mut app);
                                    app.delete_selected_input();
                                    app.status = "cut input selection".into();
                                }
                            }
                            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.paste_from_clipboard();
                            }
                            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.input_selection_start = Some(0);
                                app.input_cursor_idx = app.input.chars().count();
                            }
                            KeyCode::Left => {
                                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                                let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                                
                                if shift {
                                    if app.input_selection_start.is_none() {
                                        app.input_selection_start = Some(app.input_cursor_idx);
                                    }
                                } else {
                                    app.input_selection_start = None;
                                }
                                
                                if ctrl {
                                    let chars: Vec<char> = app.input.chars().collect();
                                    while app.input_cursor_idx > 0 && app.input_cursor_idx <= chars.len() {
                                        app.input_cursor_idx -= 1;
                                        if app.input_cursor_idx == 0 || (chars[app.input_cursor_idx - 1] == ' ' && chars[app.input_cursor_idx] != ' ') { break; }
                                            }
                                } else {
                                    app.input_cursor_idx = app.input_cursor_idx.saturating_sub(1);
                                }
                            }
                            KeyCode::Right => {
                                let chars_len = app.input.chars().count();
                                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                                let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                                
                                if shift {
                                    if app.input_selection_start.is_none() {
                                        app.input_selection_start = Some(app.input_cursor_idx);
                                    }
                                } else {
                                    app.input_selection_start = None;
                                }
                                
                                if ctrl {
                                    let chars: Vec<char> = app.input.chars().collect();
                                    while app.input_cursor_idx < chars_len {
                                        app.input_cursor_idx += 1;
                                        if app.input_cursor_idx == chars_len || (chars[app.input_cursor_idx] == ' ' && chars[app.input_cursor_idx - 1] != ' ') { break; }
                                            }
                                } else {
                                    if app.input_cursor_idx < chars_len {
                                        app.input_cursor_idx += 1;
                                    }
                                }
                            }
                            KeyCode::Home => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    if app.input_selection_start.is_none() {
                                        app.input_selection_start = Some(app.input_cursor_idx);
                                    }
                                } else {
                                    app.input_selection_start = None;
                                }
                                app.input_cursor_idx = 0;
                            }
                            KeyCode::End => {
                                let chars_len = app.input.chars().count();
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    if app.input_selection_start.is_none() {
                                        app.input_selection_start = Some(app.input_cursor_idx);
                                    }
                                } else {
                                    app.input_selection_start = None;
                                }
                                app.input_cursor_idx = chars_len;
                            }
                            KeyCode::Backspace => {
                                if !app.delete_selected_input() {
                                    if app.input_cursor_idx > 0 {
                                        let mut chars: Vec<char> = app.input.chars().collect();
                                        chars.remove(app.input_cursor_idx - 1);
                                        app.input_cursor_idx -= 1;
                                        app.input = chars.into_iter().collect();
                                    }
                                }
                            }
                            KeyCode::Delete => {
                                if !app.delete_selected_input() {
                                    let chars_len = app.input.chars().count();
                                    if app.input_cursor_idx < chars_len {
                                        let mut chars: Vec<char> = app.input.chars().collect();
                                        chars.remove(app.input_cursor_idx);
                                        app.input = chars.into_iter().collect();
                                    }
                                }
                            }
                            KeyCode::Char(c) => {
                                app.delete_selected_input();
                                let mut chars: Vec<char> = app.input.chars().collect();
                                chars.insert(app.input_cursor_idx, c);
                                app.input_cursor_idx += 1;
                                app.input = chars.into_iter().collect();
                            }
                            KeyCode::Tab => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    app.cycle_permission(&agent);
                                } else if let Some(model) = app.cycle_model(1) {
                                    set_model(&agent, session, &mut app, model);
                                }
                            }
                            KeyCode::Enter => {
                                // Shift+Enter inserts a newline; Enter sends.
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    app.delete_selected_input();
                                    let mut chars: Vec<char> = app.input.chars().collect();
                                    chars.insert(app.input_cursor_idx, '\n');
                                    app.input_cursor_idx += 1;
                                    app.input = chars.into_iter().collect();
                                    continue;
                                }

                                if in_flight.is_some() {
                                    app.status = "busy — wait for response...".into();
                                    continue;
                                }

                                let input = app.input.trim().to_string();
                                app.input.clear();
                                app.input_cursor_idx = 0;
                                app.input_selection_start = None;
                                if input.is_empty() {
                                    continue;
                                }
                                app.push_history(&input);

                                if input.starts_with("/") {
                                    match handle_slash_command(
                                        &input,
                                        &mut app,
                                        session,
                                        store,
                                        agent.clone(),
                                        ssh_manager.as_ref(),
                                        workspace.clone(),
                                        skill_loader.clone(),
                                    )
                                    .await?
                                    {
                                        CommandOutcome::Quit => should_quit = true,
                                        CommandOutcome::Continue => {}
                                    }
                                    continue;
                                }

                                app.end_stream();
                                app.ensure_newline();
                                app.output_text.push_str("> ");
                                app.output_text.push_str(&input);
                                app.output_text.push('\n');
                                app.status = "thinking...".into();

                                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                                let (approval_tx, approval_rx) =
                                    tokio::sync::mpsc::unbounded_channel::<bool>();
                                let (options_tx, options_rx) =
                                    tokio::sync::mpsc::unbounded_channel::<String>();
                                approval_resp_tx = Some(approval_tx);
                                options_resp_tx = Some(options_tx);
                                let agent_clone = agent.clone();
                                let user_msg = input.clone();
                                let session_snapshot = session.clone();
                                let handle = tokio::spawn(async move {
                                    let mut s = session_snapshot;
                                    let result = agent_clone
                                        .run_turn(
                                            &mut s,
                                            user_msg,
                                            Some(tx),
                                            Interactivity::new(
                                                ApprovalGate::new(approval_rx),
                                                OptionsGate::new(options_rx),
                                            ),
                                        )
                                        .await;
                                    (s, result)
                                });

                                app.streaming = true;
                                in_flight = Some(InFlight { handle, rx });
                            }
                            KeyCode::Up => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    continue;
                                }
                                if app.input.is_empty() {
                                    app.history_back();
                                } else {
                                    app.move_cursor_vertically(-1);
                                }
                            }
                            KeyCode::Down => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    continue;
                                }
                                if app.input.is_empty() {
                                    app.history_forward();
                                } else {
                                    app.move_cursor_vertically(1);
                                }
                            }
                            KeyCode::Esc => {
                                if in_flight.is_some() {
                                    if let Some(running) = in_flight.take() {
                                        finish_inflight(
                                            &mut app,
                                            session,
                                            store,
                                            running,
                                            true,
                                            UiChannels {
                                                approval: &mut approval_resp_tx,
                                                options: &mut options_resp_tx,
                                            },
                                        )
                                        .await;
                                    }
                                } else {
                                    app.selection = None;
                                    app.input_selection_start = None;
                                    app.show_help = false;
                                    app.close_model_picker();
                                }
                            }
                            _ => {}
                        }
                    }
                    Some(Err(e)) => {
                        return Err(e.into());
                    }
                    None => {
                        should_quit = true;
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                app.frame = app.frame.wrapping_add(1);
            }
        }
    }

    if let Some(running) = in_flight.take() {
        finish_inflight(
            &mut app,
            session,
            store,
            running,
            true,
            UiChannels {
                approval: &mut approval_resp_tx,
                options: &mut options_resp_tx,
            },
        )
        .await;
    }
    session.todos = app.todos.clone();
    let _ = store.save(session);

    if current_mouse_state {
        let _ = execute!(io::stdout(), crossterm::event::DisableMouseCapture);
    }
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

async fn handle_slash_command(
    input: &str,
    app: &mut TuiApp,
    session: &mut Session,
    store: &SessionStore,
    agent: std::sync::Arc<Agent>,
    ssh_manager: Option<&std::sync::Arc<forge_ssh::SshManager>>,
    workspace: std::path::PathBuf,
    skill_loader: std::sync::Arc<forge_tool::SkillLoader>,
) -> Result<CommandOutcome> {
    use crate::commands::SlashCommand;
    let cmd = SlashCommand::parse(input);
    match cmd {
        SlashCommand::Help => {
            app.show_help = true;
        }
        SlashCommand::Clear => {
            app.output_text.clear();
        }
        SlashCommand::Model(args) => {
            if let Some(model) = args {
                set_model(&agent, session, app, model.clone());
                app.push_output(
                    &format!("Model set to {model}"),
                    Style::default().fg(THEME.warning),
                );
            } else {
                app.push_output(
                    &format!("Current model: {}", app.model),
                    Style::default().fg(THEME.warning),
                );
                if !app.available_models.is_empty() {
                    app.push_output(
                        &format!("Available: {}", app.available_models.join(", ")),
                        Style::default().fg(THEME.text_muted),
                    );
                }
            }
        }
        SlashCommand::Tools => {
            let tools = agent.tool_names().join(", ");
            app.push_output(&format!("Tools: {tools}"), Style::default().fg(THEME.info));
        }
        SlashCommand::Resume => {
            if let Ok(Some(s)) = store.latest() {
                *session = s;
                app.session_id = session.id.clone();
                app.model = session.model.clone();
                app.push_output("Resumed last session", Style::default().fg(THEME.success));
            } else {
                app.push_output("No session to resume", Style::default().fg(THEME.error));
            }
        }
        SlashCommand::Ssh(args) => {
            if let Some(mgr) = ssh_manager {
                handle_ssh_command(args, app, mgr).await;
            } else {
                app.push_output("SSH not configured", Style::default().fg(THEME.error));
            }
        }
        SlashCommand::Debug => {
            app.push_output(
                "Debug mode: use `forge debug analyze <log>` or `forge debug start`",
                Style::default().fg(THEME.warning),
            );
        }
        SlashCommand::Parallel(tasks) => {
            if tasks.is_empty() {
                app.push_output(
                    "Usage: /parallel task1; task2; task3",
                    Style::default().fg(THEME.warning),
                );
            } else {
                app.push_output(
                    &format!("Running {} parallel tasks...", tasks.len()),
                    Style::default().fg(THEME.accent_alt),
                );
                app.status = "parallel...".into();

                let executor = forge_parallel::ParallelExecutor::new(
                    agent.clone(),
                    workspace,
                    agent.model(),
                );
                match executor.run_parallel(tasks).await {
                    Ok(results) => {
                        for task in results {
                            let status = match task.status {
                                forge_parallel::TaskStatus::Completed => "[ok]",
                                forge_parallel::TaskStatus::Failed => "[fail]",
                                _ => "[..]",
                            };
                            app.push_output(
                                &format!("{status} {}", task.description),
                                Style::default().fg(THEME.accent_alt),
                            );
                            if let Some(result) = task.result {
                                let truncated = if result.len() > 300 {
                                    format!("{}...", &result[..300])
                                } else {
                                    result
                                };
                                app.push_output(&truncated, Style::default().fg(THEME.text_muted));
                            }
                        }
                        app.status = "idle".into();
                    }
                    Err(e) => {
                        app.push_output(
                            &format!("Parallel execution failed: {e}"),
                            Style::default().fg(THEME.error),
                        );
                        app.status = "idle".into();
                    }
                }
            }
        }
        SlashCommand::Todo(args) => {
            if args.is_empty() {
                app.toggle_todo_panel();
                let state = if app.show_todo_panel { "shown" } else { "hidden" };
                app.push_output(
                    &format!("Todo panel {state} (Ctrl+T toggles)"),
                    Style::default().fg(THEME.warning),
                );
            } else {
                match args[0].as_str() {
                    "add" => {
                        let text = args[1..].join(" ");
                        if text.is_empty() {
                            app.push_output(
                                "Usage: /todo add <task>",
                                Style::default().fg(THEME.warning),
                            );
                        } else {
                            app.todo_add(&text);
                            session.todos = app.todos.clone();
                            let _ = store.save(session);
                            app.push_output(
                                &format!("Added todo: {text}"),
                                Style::default().fg(THEME.success),
                            );
                        }
                    }
                    "clear" => {
                        let n = app.todos.len();
                        app.todos.clear();
                        session.todos = app.todos.clone();
                        let _ = store.save(session);
                        app.push_output(
                            &format!("Cleared {n} todos"),
                            Style::default().fg(THEME.warning),
                        );
                    }
                    other => app.push_output(
                        &format!("Unknown todo op: {other} (add, clear)"),
                        Style::default().fg(THEME.error),
                    ),
                }
            }
        }
        SlashCommand::Skills => {
            let names = skill_loader.names();
            if names.is_empty() {
                app.push_output(
                    "No skills loaded. Set [tools].skills_dir in config.",
                    Style::default().fg(THEME.warning),
                );
            } else {
                app.push_output(
                    &format!("Skills: {}", names.join(", ")),
                    Style::default().fg(THEME.info),
                );
            }
        }
        SlashCommand::ToggleMouse => {
            app.mouse_enabled = !app.mouse_enabled;
            let state = if app.mouse_enabled { "enabled" } else { "disabled" };
            app.push_output(
                &format!("✔ Mouse capture and app-owned text selection {}", state),
                Style::default().fg(THEME.warning),
            );
        }
        SlashCommand::New => {
            let (new_session, new_id) = {
                let fresh = Session::new(session.workspace.clone(), session.model.clone());
                let new_id = fresh.id.clone();
                (fresh, new_id)
            };
            *session = new_session;
            app.session_id = new_id;
            app.output_text.clear();
            app.tool_cards.clear();
            app.selection = None;
            app.scroll_offset = None;
            app.push_output("New session started", Style::default().fg(THEME.success));
        }
        SlashCommand::Compact => {
            if session.messages.len() < 20 {
                app.push_output(
                    "Session too short to compact (need 20+ messages)",
                    Style::default().fg(THEME.warning),
                );
            } else {
                let dropped = session.messages.len() - 10;
                session.messages.drain(..dropped);
                app.push_output(
                    &format!("Compacted session (dropped {dropped} oldest messages)"),
                    Style::default().fg(THEME.warning),
                );
            }
        }
        SlashCommand::Quit => {
            return Ok(CommandOutcome::Quit);
        }
        SlashCommand::Unknown(cmd) => {
            app.push_output(
                &format!("Unknown command: {cmd}. Try /help"),
                Style::default().fg(THEME.error),
            );
        }
    }
    Ok(CommandOutcome::Continue)
}

async fn handle_ssh_command(args: Vec<String>, app: &mut TuiApp, mgr: &forge_ssh::SshManager) {
    if args.is_empty() || args[0] == "list" {
        let hosts: Vec<String> = mgr
            .list_hosts()
            .iter()
            .map(|h| format!("{} -> {}@{}:{}", h.alias, h.user, h.host, h.port))
            .collect();
        app.push_output(
            &format!("SSH hosts:\n{}", hosts.join("\n")),
            Style::default().fg(THEME.info),
        );
        return;
    }
    if args[0] == "connect" && args.len() > 1 {
        match mgr.connect(&args[1]).await {
            Ok(info) => app.push_output(
                &format!("Connected to {} ({}@{})", info.alias, info.user, info.host),
                Style::default().fg(THEME.success),
            ),
            Err(e) => app.push_output(
                &format!("Connection failed: {e}"),
                Style::default().fg(THEME.error),
            ),
        }
        return;
    }
    if args[0] == "exec" && args.len() > 2 {
        match mgr.exec(&args[1], &args[2..].join(" ")).await {
            Ok(out) => app.push_output(&out, Style::default().fg(THEME.text)),
            Err(e) => app.push_output(
                &format!("SSH exec failed: {e}"),
                Style::default().fg(THEME.error),
            ),
        }
    }
}
