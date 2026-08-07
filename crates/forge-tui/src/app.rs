use anyhow::Result;
use forge_core::{Agent, AgentEvent, Session, SessionStore};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};
use std::io;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

fn parse_line_markdown<'a>(
    line_str: &'a str,
    base_idx: usize,
    sel_start: usize,
    sel_end: usize,
    inside_code_block: bool,
    is_indicator: bool,
    default_fg: Color,
) -> Line<'a> {
    let mut spans = Vec::new();
    let is_header = line_str.starts_with("#");
    let is_prompt = line_str.starts_with("> ");
    
    let line_fg = if is_indicator {
        default_fg
    } else if inside_code_block {
        Color::LightYellow
    } else if is_prompt {
        Color::Cyan
    } else if is_header {
        Color::Cyan
    } else {
        Color::White
    };
    
    let default_modifier = if is_header || is_prompt {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };

    let mut append_styled = |text: &str, fg: Color, modifier: Modifier, start_rel_offset: usize| {
        let abs_start = base_idx + start_rel_offset;
        let abs_end = abs_start + text.len();
        let has_selection = sel_start != sel_end && abs_start < sel_end && abs_end > sel_start;
        
        if has_selection {
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
                        Style::default().bg(Color::Rgb(50, 75, 110)).fg(Color::White).add_modifier(modifier)
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
                    Style::default().bg(Color::Rgb(50, 75, 110)).fg(Color::White).add_modifier(modifier)
                } else {
                    Style::default().fg(fg).add_modifier(modifier)
                };
                spans.push(Span::styled(current_segment, style));
            }
        } else {
            spans.push(Span::styled(text.to_string(), Style::default().fg(fg).add_modifier(modifier)));
        }
    };

    if is_indicator {
        append_styled(line_str, default_fg, Modifier::empty(), 0);
    } else if inside_code_block {
        append_styled(line_str, line_fg, Modifier::empty(), 0);
    } else if is_header {
        append_styled(line_str, Color::Cyan, Modifier::BOLD, 0);
    } else if is_prompt {
        append_styled(line_str, Color::Cyan, Modifier::BOLD, 0);
    } else {
        let mut display_str = line_str;
        let mut offset = 0;
        
        let trimmed = line_str.trim_start();
        let leading_whitespace_len = line_str.len() - trimmed.len();
        
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            if leading_whitespace_len > 0 {
                append_styled(&line_str[..leading_whitespace_len], Color::White, Modifier::empty(), 0);
            }
            append_styled("• ", Color::Cyan, Modifier::BOLD, leading_whitespace_len);
            display_str = &trimmed[2..];
            offset = leading_whitespace_len + 2;
        }
        
        let chars: Vec<(usize, char)> = display_str.char_indices().collect();
        let mut i = 0;
        let mut normal_start_idx = 0;
        
        while i < chars.len() {
            if i + 1 < chars.len() && chars[i].1 == '*' && chars[i+1].1 == '*' {
                if chars[i].0 > normal_start_idx {
                    append_styled(&display_str[normal_start_idx..chars[i].0], line_fg, default_modifier, offset + normal_start_idx);
                }
                
                let mut bold_end_char_idx = i + 2;
                let mut found_bold_close = false;
                while bold_end_char_idx + 1 < chars.len() {
                    if chars[bold_end_char_idx].1 == '*' && chars[bold_end_char_idx+1].1 == '*' {
                        found_bold_close = true;
                        break;
                    }
                    bold_end_char_idx += 1;
                }
                
                if found_bold_close {
                    let content_start = chars[i+2].0;
                    let content_end = chars[bold_end_char_idx].0;
                    append_styled(&display_str[content_start..content_end], Color::Yellow, Modifier::BOLD, offset + content_start);
                    i = bold_end_char_idx + 2;
                    normal_start_idx = if i < chars.len() { chars[i].0 } else { display_str.len() };
                    continue;
                }
            }
            
            if chars[i].1 == '`' {
                if chars[i].0 > normal_start_idx {
                    append_styled(&display_str[normal_start_idx..chars[i].0], line_fg, default_modifier, offset + normal_start_idx);
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
                    let content_start = chars[i+1].0;
                    let content_end = chars[code_end_char_idx].0;
                    append_styled(&display_str[content_start..content_end], Color::Magenta, Modifier::empty(), offset + content_start);
                    i = code_end_char_idx + 1;
                    normal_start_idx = if i < chars.len() { chars[i].0 } else { display_str.len() };
                    continue;
                }
            }
            
            i += 1;
        }
        
        if normal_start_idx < display_str.len() {
            append_styled(&display_str[normal_start_idx..], line_fg, default_modifier, offset + normal_start_idx);
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

pub struct TuiApp {
    pub input: String,
    /// Plain-text output log rendered with word wrap.
    output_text: String,
    pub status: String,
    pub model: String,
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
}

struct InFlight {
    handle: JoinHandle<(Session, Result<()>)>,
    rx: UnboundedReceiver<AgentEvent>,
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

    pub fn get_styled_output_lines(&self) -> Vec<Line<'_>> {
        let mut lines = Vec::new();
        let text = &self.output_text;
        
        let (sel_start, sel_end) = match self.selection {
            Some((s, e)) => (s.min(e), s.max(e)),
            None => (0, 0),
        };
        
        let mut current_idx = 0;
        let mut inside_code_block = false;
        
        for line_str in text.split('\n') {
            let line_len = line_str.len();
            
            let trimmed = line_str.trim();
            if trimmed.starts_with("```") {
                inside_code_block = !inside_code_block;
                let line_with_sel = parse_line_markdown(line_str, current_idx, sel_start, sel_end, false, true, Color::DarkGray);
                lines.push(line_with_sel);
            } else {
                let line_with_sel = parse_line_markdown(line_str, current_idx, sel_start, sel_end, inside_code_block, false, Color::White);
                lines.push(line_with_sel);
            }
            
            current_idx += line_len + 1;
        }
        lines
    }

    pub fn get_styled_input_lines(&self) -> Vec<Line<'_>> {
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
                    Style::default().bg(Color::Rgb(50, 75, 110)).fg(Color::White)
                } else {
                    Style::default().fg(Color::White)
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
        
        let wrapped_lines = wrap_text(&self.output_text, inner_w as usize);
        if (wrapped_line_idx as usize) < wrapped_lines.len() {
            let range = &wrapped_lines[wrapped_line_idx as usize];
            let col = (x - inner_x) as usize;
            
            let line_sub = &self.output_text[range.start..range.end];
            let mut byte_offset = 0;
            for (char_idx, (b_idx, _)) in line_sub.char_indices().enumerate() {
                if char_idx == col {
                    byte_offset = b_idx;
                    break;
                }
                byte_offset = b_idx + 1;
            }
            Some(range.start + byte_offset)
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
        let total_lines = Paragraph::new(self.output_text.as_str())
            .wrap(Wrap { trim: false })
            .line_count(inner_width)
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

        self.last_output_area.set(chunks[1]);

        let status_style = if self.status == "idle" {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        };
        let header = Paragraph::new(Line::from(vec![
            Span::styled(" Forge ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw(" | "),
            Span::styled(format!("model: {}", self.model), Style::default().fg(Color::Yellow)),
            Span::raw(" | "),
            Span::styled(
                format!("session: {}", &self.session_id[..8.min(self.session_id.len())]),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(" | "),
            Span::styled(&self.status, status_style),
        ]));
        frame.render_widget(header, chunks[0]);

        let scroll_y = self.output_scroll_y(chunks[1].width, chunks[1].height);
        let output = Paragraph::new(self.get_styled_output_lines())
            .block(Block::default().borders(Borders::ALL).title(" Output "))
            .wrap(Wrap { trim: false })
            .scroll((scroll_y, 0));
        frame.render_widget(output, chunks[1]);

        let (input_row, input_col) = self.get_input_cursor_row_col();
        let input_inner_h = chunks[2].height.saturating_sub(2);
        let input_scroll_y = input_row.saturating_sub(input_inner_h.saturating_sub(1));
        
        let input_inner_w = chunks[2].width.saturating_sub(2);
        let input_scroll_x = input_col.saturating_sub(input_inner_w.saturating_sub(1));

        let input = Paragraph::new(self.get_styled_input_lines())
            .block(Block::default().borders(Borders::ALL).title(" Input "))
            .scroll((input_scroll_y, input_scroll_x));
        frame.render_widget(input, chunks[2]);

        let footer_text = if self.status == "idle" {
            " Enter: send | Tab: model | Ctrl+V: paste | Ctrl+C: copy/quit | /mouse: toggle | /help | Esc: clear sel "
        } else {
            " Esc / Ctrl+C: interrupt generation | /mouse: toggle "
        };
        let footer = Paragraph::new(footer_text)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(footer, chunks[3]);

        if self.show_help {
            self.render_help(frame, area);
        }

        if self.show_model_picker {
            self.render_model_picker(frame, area);
        }

        // Place blinking terminal cursor at the scroll-adjusted cursor coordinate
        let cursor_x = chunks[2].x + 1 + input_col.saturating_sub(input_scroll_x);
        let cursor_y = chunks[2].y + 1 + input_row.saturating_sub(input_scroll_y);
        frame.set_cursor_position((cursor_x, cursor_y));
    }

    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let help_area = centered_rect(60, 70, area);
        let help_text = vec![
            "/help      - Show this help",
            "/model     - Show or set model (/model gpt-4o)",
            "/tools     - List available tools",
            "/clear     - Clear output",
            "/debug     - Toggle debug mode",
            "/parallel  - Run parallel tasks (/parallel task1; task2)",
            "/ssh       - SSH commands (/ssh list, /ssh connect <alias>)",
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
                .style(Style::default().bg(Color::Black)),
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
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(format!("{prefix}{model}")).style(style)
            })
            .collect();
        let picker = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Select Model (Tab/Shift+Tab, Enter/Esc) ")
                .style(Style::default().bg(Color::Black)),
        );
        frame.render_widget(picker, picker_area);
    }
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
        AgentEvent::ContentDelta { text } => {
            app.append_stream(&text, Style::default().fg(Color::White));
        }
        AgentEvent::ToolCallStart { name, arguments } => {
            app.end_stream();
            let args_short = if arguments.len() > 80 {
                format!("{}...", &arguments[..80])
            } else {
                arguments
            };
            app.push_output(
                &format!("⊘ Running {name}({args_short})"),
                Style::default().fg(Color::Magenta),
            );
        }
        AgentEvent::ToolCallEnd { name, output, is_error } => {
            app.end_stream();
            if is_error {
                let truncated = if output.len() > 200 {
                    format!("{}...", &output[..200])
                } else {
                    output
                };
                app.push_output(
                    &format!("❌ {name} failed: {truncated}"),
                    Style::default().fg(Color::Red),
                );
            } else {
                let line_count = output.lines().count();
                if line_count > 1 {
                    app.push_output(
                        &format!("✔ {name} completed ({line_count} lines of output)"),
                        Style::default().fg(Color::Green),
                    );
                } else if !output.is_empty() {
                    let truncated = if output.len() > 80 {
                        format!("{}...", &output[..80])
                    } else {
                        output
                    };
                    app.push_output(
                        &format!("✔ {name} completed: {truncated}"),
                        Style::default().fg(Color::Green),
                    );
                } else {
                    app.push_output(
                        &format!("✔ {name} completed"),
                        Style::default().fg(Color::Green),
                    );
                }
            }
        }
        AgentEvent::Error { message } => {
            app.end_stream();
            app.push_output(
                &format!("[error] {message}"),
                Style::default().fg(Color::Red),
            );
        }
        AgentEvent::TokenUsage { total, .. } => {
            app.status = format!("done ({total} tokens)");
        }
        AgentEvent::Done => {
            app.end_stream();
            return true;
        }
        _ => {}
    }
    false
}

async fn finish_inflight(
    app: &mut TuiApp,
    session: &mut Session,
    store: &SessionStore,
    mut in_flight: InFlight,
    abort: bool,
) {
    while let Ok(event) = in_flight.rx.try_recv() {
        apply_agent_event(app, event);
    }

    if abort && !in_flight.handle.is_finished() {
        in_flight.handle.abort();
        app.push_output(
            "[cancelled] agent turn aborted",
            Style::default().fg(Color::Yellow),
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
                app.push_output(&format!("Error: {e}"), Style::default().fg(Color::Red));
            }
            let _ = store.save(session);
        }
        Ok(Err(e)) => {
            app.push_output(
                &format!("Agent task failed: {e}"),
                Style::default().fg(Color::Red),
            );
        }
        Err(_) => {
            in_flight.handle.abort();
            app.push_output(
                "[timeout] agent task did not finish; aborted so UI can accept input",
                Style::default().fg(Color::Yellow),
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
    let mut in_flight: Option<InFlight> = None;
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
                finish_inflight(&mut app, session, store, finished, false).await;
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
                                        if let Some(char_idx) = app.map_coordinates_to_char_idx(mouse_event.column, mouse_event.row) {
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
                                        let total_lines = Paragraph::new(app.output_text.as_str())
                                            .wrap(Wrap { trim: false })
                                            .line_count(inner_width)
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

                        match key.code {
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
                                if let Some(model) = app.cycle_model(
                                    if key.modifiers.contains(KeyModifiers::SHIFT) {
                                        -1
                                    } else {
                                        1
                                    },
                                ) {
                                    set_model(&agent, session, &mut app, model);
                                }
                            }
                            KeyCode::Enter => {
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
                                let agent_clone = agent.clone();
                                let user_msg = input.clone();
                                let session_snapshot = session.clone();
                                let handle = tokio::spawn(async move {
                                    let mut s = session_snapshot;
                                    let result = agent_clone
                                        .run_turn(&mut s, user_msg, Some(tx))
                                        .await;
                                    (s, result)
                                });

                                in_flight = Some(InFlight { handle, rx });
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
            }
        }
    }

    if let Some(running) = in_flight.take() {
        finish_inflight(&mut app, session, store, running, true).await;
    }

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
                    Style::default().fg(Color::Yellow),
                );
            } else {
                app.push_output(
                    &format!("Current model: {}", app.model),
                    Style::default().fg(Color::Yellow),
                );
                if !app.available_models.is_empty() {
                    app.push_output(
                        &format!("Available: {}", app.available_models.join(", ")),
                        Style::default().fg(Color::DarkGray),
                    );
                }
            }
        }
        SlashCommand::Tools => {
            let tools = agent.tool_names().join(", ");
            app.push_output(&format!("Tools: {tools}"), Style::default().fg(Color::Blue));
        }
        SlashCommand::Resume => {
            if let Ok(Some(s)) = store.latest() {
                *session = s;
                app.session_id = session.id.clone();
                app.model = session.model.clone();
                app.push_output("Resumed last session", Style::default().fg(Color::Green));
            } else {
                app.push_output("No session to resume", Style::default().fg(Color::Red));
            }
        }
        SlashCommand::Ssh(args) => {
            if let Some(mgr) = ssh_manager {
                handle_ssh_command(args, app, mgr).await;
            } else {
                app.push_output("SSH not configured", Style::default().fg(Color::Red));
            }
        }
        SlashCommand::Debug => {
            app.push_output(
                "Debug mode: use `forge debug analyze <log>` or `forge debug start`",
                Style::default().fg(Color::Yellow),
            );
        }
        SlashCommand::Parallel(tasks) => {
            if tasks.is_empty() {
                app.push_output(
                    "Usage: /parallel task1; task2; task3",
                    Style::default().fg(Color::Yellow),
                );
            } else {
                app.push_output(
                    &format!("Running {} parallel tasks...", tasks.len()),
                    Style::default().fg(Color::Magenta),
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
                                Style::default().fg(Color::Magenta),
                            );
                            if let Some(result) = task.result {
                                let truncated = if result.len() > 300 {
                                    format!("{}...", &result[..300])
                                } else {
                                    result
                                };
                                app.push_output(&truncated, Style::default().fg(Color::DarkGray));
                            }
                        }
                        app.status = "idle".into();
                    }
                    Err(e) => {
                        app.push_output(
                            &format!("Parallel execution failed: {e}"),
                            Style::default().fg(Color::Red),
                        );
                        app.status = "idle".into();
                    }
                }
            }
        }
        SlashCommand::Skills => {
            let names = skill_loader.names();
            if names.is_empty() {
                app.push_output(
                    "No skills loaded. Set [tools].skills_dir in config.",
                    Style::default().fg(Color::Yellow),
                );
            } else {
                app.push_output(
                    &format!("Skills: {}", names.join(", ")),
                    Style::default().fg(Color::Blue),
                );
            }
        }
        SlashCommand::ToggleMouse => {
            app.mouse_enabled = !app.mouse_enabled;
            let state = if app.mouse_enabled { "enabled" } else { "disabled" };
            app.push_output(
                &format!("✔ Mouse capture and app-owned text selection {}", state),
                Style::default().fg(Color::Yellow),
            );
        }
        SlashCommand::Quit => {
            return Ok(CommandOutcome::Quit);
        }
        SlashCommand::Unknown(cmd) => {
            app.push_output(
                &format!("Unknown command: {cmd}. Try /help"),
                Style::default().fg(Color::Red),
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
            Style::default().fg(Color::Blue),
        );
        return;
    }
    if args[0] == "connect" && args.len() > 1 {
        match mgr.connect(&args[1]).await {
            Ok(info) => app.push_output(
                &format!("Connected to {} ({}@{})", info.alias, info.user, info.host),
                Style::default().fg(Color::Green),
            ),
            Err(e) => app.push_output(
                &format!("Connection failed: {e}"),
                Style::default().fg(Color::Red),
            ),
        }
        return;
    }
    if args[0] == "exec" && args.len() > 2 {
        match mgr.exec(&args[1], &args[2..].join(" ")).await {
            Ok(out) => app.push_output(&out, Style::default().fg(Color::White)),
            Err(e) => app.push_output(
                &format!("SSH exec failed: {e}"),
                Style::default().fg(Color::Red),
            ),
        }
    }
}
