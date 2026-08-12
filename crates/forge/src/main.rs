mod debug;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use forge_config::ForgeConfig;
use forge_core::{Agent, Interactivity, Session, SessionStore};
use forge_mcp::register_mcp_tools;
use forge_parallel::ParallelExecutor;
use forge_safety::{AuditLogger, Sandbox, WorkspaceTrust};
use forge_ssh::{register_ssh_tools, SshManager};
use forge_tool::{SkillLoader, ToolRegistry};
use forge_tui::run_tui;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Parser)]
#[command(name = "forge", about = "Local-first terminal coding agent", version)]
struct Cli {
    /// Path to config file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Workspace directory
    #[arg(short, long, default_value = ".")]
    workspace: PathBuf,

    /// Model override
    #[arg(short, long)]
    model: Option<String>,

    /// Trust workspace without prompting
    #[arg(long)]
    trust: bool,

    /// Full-auto mode (skip destructive confirmations)
    #[arg(long)]
    full_auto: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive TUI mode (default)
    Chat {
        /// Resume session by ID
        #[arg(long)]
        session: Option<String>,
    },
    /// Headless single-task execution
    Exec {
        /// Task prompt
        prompt: String,
        /// Output format
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Run parallel sub-agents
    Parallel {
        /// Tasks separated by semicolons
        #[arg(short, long)]
        tasks: String,
    },
    /// Session management
    Sessions {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// SSH host management
    Ssh {
        #[command(subcommand)]
        action: SshAction,
    },
    /// Initialize default config
    Init,
    /// Debug a program
    Debug {
        #[command(subcommand)]
        action: DebugAction,
    },
}

#[derive(Subcommand)]
enum SessionAction {
    List,
    Delete { id: String },
}

#[derive(Subcommand)]
enum SshAction {
    List,
    Connect { alias: String },
    Exec { alias: String, command: String },
}

#[derive(Subcommand)]
enum DebugAction {
    /// Analyze error output and suggest fixes
    Analyze { log_file: PathBuf },
    /// Start lldb/gdb wrapper
    Start {
        #[arg(long, default_value = "lldb")]
        debugger: String,
        program: PathBuf,
        #[arg(last = true)]
        args: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI first so TUI mode can avoid writing logs to the terminal.
    // stderr logging during the alternate screen corrupts ratatui's diff buffer
    // (ghost text like "INFO forge::..." stuck in the input area).
    let cli = Cli::parse();
    init_tracing(uses_tui(&cli))?;

    let config = ForgeConfig::load_or_default(cli.config.as_deref())?;

    if let Some(Commands::Init) = &cli.command {
        let path = ForgeConfig::default_config_path();
        if path.exists() {
            println!("Config already exists at {}", path.display());
        } else {
            ForgeConfig::default().save(&path)?;
            println!("Created default config at {}", path.display());
        }
        return Ok(());
    }

    let workspace = cli
        .workspace
        .canonicalize()
        .context("resolving workspace")?;

    let trust = WorkspaceTrust::new();
    if cli.trust {
        trust.trust(&workspace)?;
    } else {
        trust.require_trust(&workspace, config.safety.workspace_trust_required)?;
    }

    let full_auto = cli.full_auto || config.agent.full_auto;
    let sandbox = Arc::new(Sandbox::new(workspace.clone(), &config.safety, full_auto));
    let audit = Arc::new(AuditLogger::new(&config.safety));

    let skills_dir = config.tools.skills_dir.clone().or_else(|| {
        let local = workspace.join("skills");
        if local.exists() {
            Some(local)
        } else {
            Some(dirs::home_dir()?.join(".forge").join("skills"))
        }
    });
    let skill_loader = Arc::new(SkillLoader::load(skills_dir.as_deref()));
    let ssh_manager = Arc::new(SshManager::new(config.ssh.hosts.clone()));

    let mut tool_registry = ToolRegistry::new(
        sandbox,
        audit.clone(),
        &config.tools,
        Some(skill_loader.clone()),
    );
    let mcp_tools = register_mcp_tools(&mut tool_registry, &config.mcp).await?;
    if !mcp_tools.is_empty() {
        tracing::info!(count = mcp_tools.len(), "registered MCP tools");
    }
    register_ssh_tools(
        &mut tool_registry,
        ssh_manager.clone(),
        config.ssh.allowed_commands.clone(),
    );
    if !config.ssh.hosts.is_empty() {
        tracing::info!(
            count = config.ssh.hosts.len(),
            "registered SSH remote tools"
        );
    } else {
        tracing::info!("registered SSH config alias tool");
    }
    let tools = Arc::new(tool_registry);

    let mut forge_config = config.clone();
    let skills_ctx = skill_loader.system_context();
    if !skills_ctx.is_empty() {
        forge_config.agent.system_prompt =
            format!("{}\n\n{}", forge_config.agent.system_prompt, skills_ctx);
    }

    let agent = Agent::from_forge_config(&forge_config, tools);
    if let Some(model) = &cli.model {
        agent.set_model(model.clone());
    }

    // Audit hook: log all tool invocations
    let audit_for_hooks = audit.clone();
    agent.hooks().register(move |ctx| {
        use forge_core::HookPhase;
        if ctx.phase == HookPhase::PostTool {
            if let (Some(name), Some(output)) = (&ctx.tool_name, &ctx.output) {
                let ok = !ctx.is_error.unwrap_or(false);
                audit_for_hooks.log(
                    name,
                    "hook",
                    serde_json::json!({ "output_len": output.len() }),
                    ok,
                );
            }
        }
    });

    let agent = Arc::new(agent);
    let store = SessionStore::new(config.session.storage_dir.clone())?;

    match cli.command {
        None => {
            let mut session = Session::new(workspace.clone(), agent.model());
            run_tui(
                agent.clone(),
                &mut session,
                &store,
                Some(ssh_manager),
                workspace.clone(),
                skill_loader.clone(),
                forge_config.available_models(),
            )
            .await?;
        }
        Some(Commands::Chat {
            session: session_id,
        }) => {
            let mut session = if let Some(id) = session_id {
                store.load(&id)?
            } else {
                Session::new(workspace.clone(), agent.model().to_string())
            };
            run_tui(
                agent.clone(),
                &mut session,
                &store,
                Some(ssh_manager),
                workspace.clone(),
                skill_loader.clone(),
                forge_config.available_models(),
            )
            .await?;
        }
        Some(Commands::Exec { prompt, format }) => {
            let mut session = Session::new(workspace, agent.model());
            // Preserve complete native tool-call envelopes in headless mode.
            agent.set_stream(false);
            let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
            agent
                .run_turn(&mut session, prompt, Some(event_tx), Interactivity::none())
                .await?;
            while let Ok(event) = event_rx.try_recv() {
                use forge_core::AgentEvent;
                match event {
                    AgentEvent::ToolCallStart { name, arguments } => {
                        eprintln!("[tool:start] {name} {arguments}");
                    }
                    AgentEvent::ToolCallEnd {
                        name,
                        output,
                        is_error,
                    } => {
                        eprintln!("[tool:end] {name} error={is_error}\n{output}");
                    }
                    AgentEvent::Error { message } => eprintln!("[agent:error] {message}"),
                    AgentEvent::Retrying { attempt, error } => {
                        eprintln!("[retry:{attempt}] {error}");
                    }
                    _ => {}
                }
            }
            if config.session.auto_save {
                store.save(&session)?;
            }
            print_exec_output(&session, &format);
        }
        Some(Commands::Parallel { tasks }) => {
            let task_list: Vec<String> = tasks
                .split(';')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let executor = ParallelExecutor::new(agent.clone(), workspace, agent.model());
            let results = executor.run_parallel(task_list).await?;
            for task in results {
                println!(
                    "=== {} [{}] ===",
                    task.description,
                    format_status(&task.status)
                );
                if let Some(result) = task.result {
                    println!("{result}");
                }
            }
        }
        Some(Commands::Sessions { action }) => match action {
            SessionAction::List => {
                for meta in store.list()? {
                    println!(
                        "{}  {}  ({})  {} msgs  {}",
                        &meta.id[..8],
                        meta.title,
                        meta.model,
                        meta.message_count,
                        meta.updated_at.format("%Y-%m-%d %H:%M")
                    );
                }
            }
            SessionAction::Delete { id } => {
                store.delete(&id)?;
                println!("Deleted session {id}");
            }
        },
        Some(Commands::Ssh { action }) => match action {
            SshAction::List => {
                for host in ssh_manager.list_hosts() {
                    println!(
                        "{} -> {}@{}:{}",
                        host.alias, host.user, host.host, host.port
                    );
                }
            }
            SshAction::Connect { alias } => {
                let info = ssh_manager.connect(&alias).await?;
                println!("Connected to {} ({}@{})", info.alias, info.user, info.host);
            }
            SshAction::Exec { alias, command } => {
                let output = ssh_manager.exec(&alias, &command).await?;
                println!("{output}");
            }
        },
        Some(Commands::Debug { action }) => match action {
            DebugAction::Analyze { log_file } => {
                let content = std::fs::read_to_string(&log_file)?;
                let analysis = debug::analyze_log(&content);
                println!("{analysis}");
            }
            DebugAction::Start {
                debugger,
                program,
                args,
            } => {
                debug::start_debugger(&debugger, &program, &args).await?;
            }
        },
        Some(Commands::Init) => unreachable!(),
    }

    Ok(())
}

/// Interactive chat owns the terminal; all other subcommands print to stdout/stderr.
fn uses_tui(cli: &Cli) -> bool {
    matches!(cli.command, None | Some(Commands::Chat { .. }))
}

fn init_tracing(tui: bool) -> Result<()> {
    let filter = EnvFilter::from_default_env().add_directive("forge=info".parse()?);
    if tui {
        let dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".forge");
        std::fs::create_dir_all(&dir)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("forge.log"))?;
        fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init();
    } else {
        fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    }
    Ok(())
}

fn print_exec_output(session: &Session, format: &str) {
    use forge_provider::Message;
    let last = session.messages.iter().rev().find_map(|m| match m {
        Message::Assistant { content, .. } => content.clone(),
        _ => None,
    });
    match format {
        "json" => {
            println!(
                "{}",
                serde_json::to_string_pretty(session).unwrap_or_default()
            );
        }
        _ => {
            if let Some(content) = last {
                println!("{content}");
            }
        }
    }
}

fn format_status(status: &forge_parallel::TaskStatus) -> &'static str {
    use forge_parallel::TaskStatus;
    match status {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
    }
}
