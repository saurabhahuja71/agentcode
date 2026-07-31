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
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

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
        }
    }

    fn ensure_newline(&mut self) {
        if !self.output_text.is_empty() && !self.output_text.ends_with('\n') {
            self.output_text.push('\n');
        }
    }

    pub fn push_output(&mut self, text: &str, _style: Style) {
        if text.is_empty() {
            return;
        }
        self.ensure_newline();
        self.output_text.push_str(text);
        if !text.ends_with('\n') {
            self.output_text.push('\n');
        }
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
        if !self.output_text.is_empty() && !self.output_text.ends_with('\n') {
            self.output_text.push('\n');
        }
    }

    fn paste_into_input(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\n' || ch == '\r' {
                break;
            }
            self.input.push(ch);
        }
    }

    fn copy_output(&mut self) -> bool {
        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(self.output_text.clone())) {
            Ok(()) => {
                self.status = "copied output".into();
                true
            }
            Err(_) => {
                self.status = "clipboard unavailable".into();
                false
            }
        }
    }

    fn paste_from_clipboard(&mut self) -> bool {
        match arboard::Clipboard::new().and_then(|mut cb| cb.get_text()) {
            Ok(text) => {
                self.paste_into_input(&text);
                true
            }
            Err(_) => false,
        }
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
            Span::styled(&self.status, Style::default().fg(Color::Green)),
        ]));
        frame.render_widget(header, chunks[0]);

        let scroll_y = self.output_scroll_y(chunks[1].width, chunks[1].height);
        let output = Paragraph::new(self.output_text.as_str())
            .block(Block::default().borders(Borders::ALL).title(" Output "))
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false })
            .scroll((scroll_y, 0));
        frame.render_widget(output, chunks[1]);

        // Horizontal scroll so long input stays usable; empty input always shows blank.
        let input_inner_w = chunks[2].width.saturating_sub(2) as usize;
        let input_scroll_x = if input_inner_w == 0 {
            0u16
        } else {
            self.input
                .chars()
                .count()
                .saturating_sub(input_inner_w.saturating_sub(1))
                .min(u16::MAX as usize) as u16
        };
        let input = Paragraph::new(self.input.as_str())
            .block(Block::default().borders(Borders::ALL).title(" Input "))
            .style(Style::default().fg(Color::White))
            .scroll((0, input_scroll_x));
        frame.render_widget(input, chunks[2]);

        let footer = Paragraph::new(
            " Enter: send | Tab: model | Ctrl+V: paste | Ctrl+Shift+C: copy output | /help | Ctrl+C: quit ",
        )
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(footer, chunks[3]);

        if self.show_help {
            self.render_help(frame, area);
        }

        if self.show_model_picker {
            self.render_model_picker(frame, area);
        }
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
            app.push_output(
                &format!("[tool] {name}({arguments})"),
                Style::default().fg(Color::Magenta),
            );
        }
        AgentEvent::ToolCallEnd { name, output, is_error } => {
            app.end_stream();
            let truncated = if output.len() > 500 {
                format!("{}...", &output[..500])
            } else {
                output
            };
            let label = if is_error { "[error]" } else { "[ok]" };
            app.push_output(
                &format!("{label} {name}: {truncated}"),
                Style::default().fg(if is_error { Color::Red } else { Color::Green }),
            );
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
            DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
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
        EnableMouseCapture,
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

    while !should_quit {
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

        // Wake on key/paste OR a short tick so streaming redraws stay live.
        tokio::select! {
            maybe = events.next() => {
                match maybe {
                    Some(Ok(event)) => {
                        if let Event::Paste(text) = &event {
                            app.paste_into_input(text);
                            continue;
                        }

                        let Event::Key(key) = event else {
                            continue;
                        };
                        // Ignore key release/repeat so we don't double-insert on
                        // terminals that emit Kitty/Windows keyboard events.
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
                            KeyCode::Char('v')
                                if key.modifiers.contains(KeyModifiers::CONTROL)
                                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
                            {
                                app.paste_from_clipboard();
                            }
                            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.paste_from_clipboard();
                            }
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                should_quit = true;
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
                                if input.is_empty() {
                                    continue;
                                }

                                if input.starts_with('/') {
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
                            KeyCode::Char(c) => {
                                app.input.push(c);
                            }
                            KeyCode::Backspace => {
                                app.input.pop();
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
                // Tick: loop redraws and drains agent events above.
            }
        }
    }

    if let Some(running) = in_flight.take() {
        finish_inflight(&mut app, session, store, running, true).await;
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        DisableMouseCapture
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
