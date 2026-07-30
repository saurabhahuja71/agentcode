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
    pub output_lines: Vec<Line<'static>>,
    pub status: String,
    pub model: String,
    pub session_id: String,
    pub scroll: u16,
    pub show_help: bool,
}

struct InFlight {
    handle: JoinHandle<(Session, Result<()>)>,
    rx: UnboundedReceiver<AgentEvent>,
}

impl TuiApp {
    pub fn new(session: &Session) -> Self {
        Self {
            input: String::new(),
            output_lines: vec![Line::from(Span::styled(
                "Forge ready. Type a message or /help for commands.",
                Style::default().fg(Color::DarkGray),
            ))],
            status: "idle".into(),
            model: session.model.clone(),
            session_id: session.id.clone(),
            scroll: 0,
            show_help: false,
        }
    }

    pub fn push_output(&mut self, text: &str, style: Style) {
        for line in text.lines() {
            self.output_lines
                .push(Line::from(Span::styled(line.to_string(), style)));
        }
        self.scroll = self.scroll.saturating_add(text.lines().count() as u16);
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

        let output = Paragraph::new(self.output_lines.clone())
            .block(Block::default().borders(Borders::ALL).title(" Output "))
            .wrap(Wrap { trim: false })
            .scroll((self.scroll, 0));
        frame.render_widget(output, chunks[1]);

        let input = Paragraph::new(self.input.as_str())
            .block(Block::default().borders(Borders::ALL).title(" Input "))
            .style(Style::default().fg(Color::White));
        frame.render_widget(input, chunks[2]);

        let footer = Paragraph::new(" Enter: send | /help: commands | Ctrl+C: quit ")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(footer, chunks[3]);

        if self.show_help {
            self.render_help(frame, area);
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

fn apply_agent_event(app: &mut TuiApp, event: AgentEvent) -> bool {
    match event {
        AgentEvent::ContentDelta { text } => {
            app.push_output(&text, Style::default().fg(Color::White));
        }
        AgentEvent::ToolCallStart { name, arguments } => {
            app.push_output(
                &format!("\n⚙ {name}({arguments})"),
                Style::default().fg(Color::Magenta),
            );
        }
        AgentEvent::ToolCallEnd { name, output, is_error } => {
            let color = if is_error { Color::Red } else { Color::Green };
            let truncated = if output.len() > 500 {
                format!("{}...", &output[..500])
            } else {
                output
            };
            app.push_output(
                &format!("\n✓ {name}: {truncated}"),
                Style::default().fg(color),
            );
        }
        AgentEvent::Error { message } => {
            app.push_output(
                &format!("\n✗ {message}"),
                Style::default().fg(Color::Red),
            );
        }
        AgentEvent::TokenUsage { total, .. } => {
            app.status = format!("done ({total} tokens)");
        }
        AgentEvent::Done => return true,
        _ => {}
    }
    false
}

async fn finish_inflight(
    app: &mut TuiApp,
    session: &mut Session,
    store: &SessionStore,
    mut in_flight: InFlight,
) {
    while let Ok(event) = in_flight.rx.try_recv() {
        if apply_agent_event(app, event) {
            break;
        }
    }

    match in_flight.handle.await {
        Ok((updated_session, result)) => {
            while let Ok(event) = in_flight.rx.try_recv() {
                apply_agent_event(app, event);
            }
            *session = updated_session;
            if let Err(e) = result {
                app.push_output(&format!("Error: {e}"), Style::default().fg(Color::Red));
            }
            let _ = store.save(session);
        }
        Err(e) => {
            app.push_output(
                &format!("Agent task failed: {e}"),
                Style::default().fg(Color::Red),
            );
        }
    }
    app.status = "idle".into();
}

pub async fn run_tui(
    agent: std::sync::Arc<Agent>,
    session: &mut Session,
    store: &SessionStore,
    ssh_manager: Option<std::sync::Arc<forge_ssh::SshManager>>,
    workspace: std::path::PathBuf,
    skill_loader: std::sync::Arc<forge_tool::SkillLoader>,
) -> Result<()> {
    use crossterm::{
        event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let mut app = TuiApp::new(session);
    let mut in_flight: Option<InFlight> = None;

    loop {
        terminal.draw(|f| app.render(f))?;

        // Drain streaming events without blocking the UI loop
        if let Some(ref mut running) = in_flight {
            let mut done = false;
            while let Ok(event) = running.rx.try_recv() {
                if apply_agent_event(&mut app, event) {
                    done = true;
                    break;
                }
            }
            if done || running.handle.is_finished() {
                let finished = in_flight.take().expect("in_flight");
                finish_inflight(&mut app, session, store, finished).await;
            }
        }

        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
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
                            handle_slash_command(
                                &input,
                                &mut app,
                                session,
                                store,
                                agent.clone(),
                                ssh_manager.as_ref(),
                                workspace.clone(),
                                skill_loader.clone(),
                            )
                            .await?;
                            continue;
                        }

                        app.push_output(&format!("> {input}"), Style::default().fg(Color::Cyan));
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
                        app.show_help = false;
                    }
                    _ => {}
                }
            }
        } else {
            // Yield so the agent task can run while we poll
            tokio::task::yield_now().await;
        }
    }

    if let Some(running) = in_flight.take() {
        finish_inflight(&mut app, session, store, running).await;
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
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
) -> Result<()> {
    use crate::commands::SlashCommand;
    let cmd = SlashCommand::parse(input);
    match cmd {
        SlashCommand::Help => {
            app.show_help = true;
        }
        SlashCommand::Clear => {
            app.output_lines.clear();
        }
        SlashCommand::Model(args) => {
            if let Some(model) = args {
                session.model = model.clone();
                app.model = model.clone();
                app.push_output(
                    &format!("Model set to {model}"),
                    Style::default().fg(Color::Yellow),
                );
            } else {
                app.push_output(
                    &format!("Current model: {}", app.model),
                    Style::default().fg(Color::Yellow),
                );
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
                    agent.model().to_string(),
                );
                match executor.run_parallel(tasks).await {
                    Ok(results) => {
                        for task in results {
                            let status = match task.status {
                                forge_parallel::TaskStatus::Completed => "✓",
                                forge_parallel::TaskStatus::Failed => "✗",
                                _ => "·",
                            };
                            app.push_output(
                                &format!("\n{status} {}", task.description),
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
            app.push_output("Use Ctrl+C to quit", Style::default().fg(Color::DarkGray));
        }
        SlashCommand::Unknown(cmd) => {
            app.push_output(
                &format!("Unknown command: {cmd}. Try /help"),
                Style::default().fg(Color::Red),
            );
        }
    }
    Ok(())
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
