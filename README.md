# Forge

**Forge** is a local-first, production-oriented terminal coding agent written in Rust. It combines a streaming LLM agent loop, safe file/shell tools, parallel sub-agents, SSH remote execution, and a ratatui TUI — all driven by TOML configuration.

## Architecture

```mermaid
flowchart TB
    subgraph CLI["forge CLI"]
        TUI[TUI - ratatui]
        Exec[Headless exec]
        Parallel[Parallel runner]
    end

    subgraph Core["forge-core"]
        Agent[Agent Loop]
        Session[Session Store]
        Summarize[Context Summarizer]
    end

    subgraph Providers["forge-provider"]
        Router[Provider Router]
        OpenAI[OpenAI-compatible]
        Ollama[Ollama]
    end

    subgraph Tools["forge-tool"]
        File[read/write/edit/list]
        Shell[Shell executor]
        Search[grep/glob]
        Git[git status/diff]
    end

    subgraph Safety["forge-safety"]
        Sandbox[Path + command sandbox]
        Trust[Workspace trust]
        Audit[Audit log]
    end

    subgraph Remote["forge-ssh"]
        SSH[SshManager]
    end

    TUI --> Agent
    Exec --> Agent
    Parallel --> Agent
    Agent --> Router
    Router --> OpenAI
    Router --> Ollama
    Agent --> Tools
    Tools --> Sandbox
    Tools --> Audit
    TUI --> SSH
    Agent --> Session
    Agent --> Summarize
```

### Crate layout

| Crate | Responsibility |
|-------|----------------|
| `forge` | CLI entrypoint (`forge`, `forge exec`, etc.) |
| `forge-config` | TOML configuration loading/saving |
| `forge-core` | Agent loop, sessions, context management |
| `forge-provider` | LLM providers + failover router |
| `forge-tool` | Tool registry and implementations |
| `forge-safety` | Sandbox, workspace trust, audit logging |
| `forge-parallel` | Parallel sub-agent execution |
| `forge-ssh` | SSH host management and remote exec |
| `forge-tui` | Interactive terminal UI + slash commands |

### Agent loop

1. User message appended to session
2. Context summarized if over threshold
3. LLM called with tool definitions (streaming)
4. Tool calls executed concurrently per turn
5. Results fed back until no more tool calls or max turns
6. Session auto-saved to `~/.forge/sessions/`

## Installation

### Prebuilt binary (Linux x86_64) — recommended

Download the latest release from
[GitHub Releases](https://github.com/saurabhahuja71/agentcode/releases):

```bash
# Binary
curl -fsSL -o forge \
  https://github.com/saurabhahuja71/agentcode/releases/download/v1.1.0/forge
chmod +x forge
sudo mv forge /usr/local/bin/

# Or tarball + checksums
curl -fsSL -O https://github.com/saurabhahuja71/agentcode/releases/download/v1.1.0/forge-linux-x86_64.tar.gz
curl -fsSL -O https://github.com/saurabhahuja71/agentcode/releases/download/v1.1.0/SHA256SUMS
sha256sum -c SHA256SUMS
tar xzf forge-linux-x86_64.tar.gz
chmod +x forge && sudo mv forge /usr/local/bin/

forge --version   # forge 1.1.0
forge init        # writes ~/.forge/config.toml
```

**Runtime:** Linux x86_64 (glibc). Optional: `git` (worktrees / git tools), `ssh` (remote tools).

### Build from source

**Requirements:** Rust 1.75+, `bash`, `git` (optional), `ssh` (for remote features)

```bash
git clone https://github.com/saurabhahuja71/agentcode.git
cd agentcode
cargo build --release
cargo install --path crates/forge
forge init
```

Config is written to `~/.forge/config.toml`. See [`config.example.toml`](config.example.toml) for all options. Local stacks (Ollama / SGLang) are documented below and in that example file.

## Quick start

```bash
# Trust workspace and start interactive TUI
forge --trust

# Headless single task
forge exec "list all Rust files and summarize the project structure"

# Override model
forge --model gpt-4o --trust

# Resume a session
forge chat --session <session-id>
```

### Provider setup

Forge talks to **any OpenAI-compatible Chat Completions API**, plus a first-class
**Ollama** provider. Providers are tried in `priority` order (lower = first) with
automatic failover. Point `base_url` at your server; Forge appends
`/v1/chat/completions` unless the URL already ends with `/v1` or
`/chat/completions`.

#### Ollama (default local setup)

Works well for daily coding with GGUF models (Q4_K_M / Q5_K_M are good defaults).

```bash
# Install & pull a model (local or remote Ollama host)
ollama pull qwen2.5-coder:32b
# or: ollama pull llama3.2
```

`~/.forge/config.toml` (or pass `-c path/to.toml`):

```toml
[agent]
model = "qwen2.5-coder:32b"
temperature = 0.2
max_tokens = 4096
max_turns = 50

[[providers]]
name = "ollama"
kind = "ollama"
# Local default; use host:port if Ollama is remote
base_url = "http://127.0.0.1:11434"
models = ["qwen2.5-coder:32b", "llama3.2", "codellama"]
enabled = true
priority = 10
```

```bash
forge --trust
# or one-shot with an alternate config
forge --config config.example.toml --trust exec "summarize this repo"
```

Tips:
- Match `[agent].model` to a name Ollama actually serves (`ollama list`).
- For a second Ollama on another port: set `base_url = "http://127.0.0.1:11435"`.
- Smaller context windows (e.g. 32k) keep long agent sessions stable — see `[session]` in the example config.

#### SGLang (OpenAI-compatible, high throughput)

SGLang exposes an OpenAI-compatible API. Use `kind = "openai_compatible"`, leave
`models = []` to accept whatever model name the server registered, and set a
dummy API key if the server expects one.

```toml
[agent]
model = "qwen2.5-coder-32b-q4_k_m.gguf"   # must match SGLang --model-path / served name
temperature = 0.2
max_tokens = 4096
max_turns = 50

[[providers]]
name = "sglang"
kind = "openai_compatible"
# Server root only — Forge adds /v1/chat/completions
base_url = "http://127.0.0.1:30000"
api_key = "sglang"
models = []          # empty = accept any model name
enabled = true
priority = 1
```

```bash
# After SGLang is listening (local or via SSH tunnel to :30000)
forge --config ~/.forge/config.toml --trust
# smoke test
forge --trust exec "reply with the single word: pong"
```

Tips:
- Prefer **AWQ INT4** or **BF16 safetensors** on NVIDIA GPUs when quality matters; GGUF Q4 is fine for throughput.
- If the server is remote, open an SSH tunnel and keep `base_url` on localhost, e.g.  
  `ssh -N -L 30000:127.0.0.1:30000 user@gpu-host`
- Optional Ollama fallback (higher `priority` number = tried later):

```toml
[[providers]]
name = "ollama-fallback"
kind = "ollama"
base_url = "http://127.0.0.1:11434"
models = ["qwen2.5-coder:32b"]
enabled = true
priority = 20
```

#### OpenAI / Azure / Groq / Together / etc.

```toml
[[providers]]
name = "openai"
kind = "openai_compatible"
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
models = ["gpt-4o"]
enabled = true
priority = 5

[agent]
model = "gpt-4o"
```

Set `OPENAI_API_KEY` (or the env name you put in `api_key_env`) in your environment.

## Slash commands (TUI)

| Command | Description |
|---------|-------------|
| `/help` | Show command help |
| `/model [name]` | Show or set model |
| `/mode ask|allow|plan` | Change permission mode |
| `/tools` | List available tools |
| `/clear` | Clear output pane |
| `/new` | Start a fresh session |
| `/compact` | Drop oldest messages when the session is long |
| `/todo` | Toggle the todo panel (`/todo add <task>`, `/todo clear`) |
| `/resume` | Resume last session |
| `/ssh list` | List SSH hosts |
| `/ssh connect <alias>` | Connect to host |
| `/ssh exec <alias> <cmd>` | Run remote command |
| `/parallel task1; task2` | Queue parallel tasks |
| `/debug` | Debug mode info |
| `/skills` | List loaded skills |
| `/toggle-mouse` | Enable/disable mouse capture + text selection (`F6` or `F8`) |
| `/workspace` | Toggle workspace path restriction |

The `http_request` tool is enabled by default and supports bounded GET, POST,
PUT, PATCH, and DELETE requests to explicit `http://` or `https://` URLs. Set
`[tools].http = false` to disable it.

## TUI features

- **Streaming transcript** with word-wrap, visible scrollbar, mouse wheel scrolling, `PageUp`/`PageDown` scrolling, and click-to-expand/collapse tool cards (long outputs are auto-folded).
- **Text selection & copy:** drag with the mouse to select, `Ctrl+Shift+C` / `Ctrl+C` copies; double/triple click selects word/line. `Ctrl+X` cuts the input selection, `Ctrl+V` pastes.
- **Multiline input:** `Shift+Enter` inserts a newline, `↑/↓` recall history, `Ctrl+A` selects all in the input.
- **Tool approvals:** Forge starts in `allow` mode by default. Use `/mode ask` for destructive-command prompts, or `/mode plan` to approve proposed changes before execution.
- **Reasoning display:** providers that stream `reasoning_content` render a timed `💭 Thought: 14.3s` block with the model's reasoning, collapsed by default.
- **Options picker:** when the agent calls `ask_options`, a numbered picker appears (`↑/↓` + `Enter`, or click); typing a printable character switches to a custom answer, `Esc` dismisses.
- **Todo side panel:** live task list on the right. `Ctrl+T` opens/focuses/closes it, `Space` toggles an item, `d` deletes, `[`/`]` resize while focused, mouse clicks toggle. Todos persist with the session and sync with the agent's `todo` tool.
- **Rich status bar:** spinner, live token count + context usage %, permission mode, and mouse state. Mouse capture starts off; use `F6`, `F8`, or `/mouse` to enable it. Permission mode cycles with `Shift+Tab`; `Ctrl+Q` quits immediately.
- **Focus management:** the todo panel can hold keyboard focus (`Tab`/`Esc` returns to input); the input stays reachable at all times.

## CLI reference

```bash
forge                          # Interactive TUI
forge exec "task"              # Headless execution
forge exec "task" --format json
forge parallel --tasks "fix tests; update docs"
forge sessions list
forge sessions delete <id>
forge ssh list
forge ssh connect prod
forge ssh exec prod "uptime"
forge debug analyze error.log
forge debug start -- ./myapp --arg
forge init                     # Create ~/.forge/config.toml
```

### Flags

| Flag | Description |
|------|-------------|
| `-c, --config <path>` | Config file path |
| `-w, --workspace <dir>` | Workspace root (default: `.`) |
| `-m, --model <name>` | Model override |
| `--trust` | Trust workspace without prompt |
| `--full-auto` | Skip destructive action confirmations |

## Tools

| Tool | Description |
|------|-------------|
| `read_file` | Read file with optional line offset/limit |
| `write_file` | Write/create files |
| `edit_file` | Surgical search-and-replace |
| `list_dir` | List directory (optional recursive) |
| `grep` | Regex search across files |
| `glob` | Find files by glob pattern |
| `project_index` | Build project structure + symbol index |
| `search_codebase` | Relevance search across indexed project |
| `shell` | Execute sandboxed shell commands |
| `git_status` | Git working tree status |
| `git_diff` | Git diff (optional path, staged) |
| `git_log` | Recent commit history |
| `git_commit` | Stage all and commit with message |
| `code_outline` | Extract functions/structs/classes from source files |
| `read_skill` | Load a skill workflow by name |
| `todo` | Add/complete/reopen/remove tasks (live session todo list) |
| `ask_options` | Ask the user to pick from a list of options (TUI picker) |
| `remote_exec` | Run command on SSH host (when configured) |
| `remote_read_file` | Read file from SSH host |
| `remote_list_dir` | List directory on SSH host |

## MCP integration

Configure MCP servers in `~/.forge/config.toml`. Tools are auto-registered as `mcp_<server>_<tool>`:

```toml
[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/workspace"]
```

## Safety model

- **Workspace trust:** Untrusted workspaces are blocked until `--trust` or explicit trust
- **Path sandbox:** File operations restricted to workspace by default
- **Command allow-list:** Only approved commands can run via `shell` tool
- **Destructive confirmation:** `rm`, `git reset --hard`, etc. require confirmation (unless `--full-auto`)
- **Audit log:** All tool calls logged to `~/.forge/audit.log`

## Parallel execution

```bash
forge parallel --tasks "add unit tests for auth; document API endpoints; fix clippy warnings"
```

Each task runs in an isolated agent session concurrently. In git repos, tasks automatically use **git worktrees** for safe parallel edits.

## SSH

Configure hosts in `~/.forge/config.toml`:

```toml
[[ssh.hosts]]
alias = "prod"
host = "10.0.0.1"
port = 22
user = "deploy"
identity_file = "~/.ssh/id_ed25519"
```

```bash
forge ssh connect prod
forge ssh exec prod "journalctl -u myapp -n 50"
```

## Debugging

```bash
# Analyze error logs
forge debug analyze /var/log/app.log

# Launch interactive debugger
forge debug start -- ./target/debug/myapp
forge debug start --debugger gdb -- ./myapp
```

## Configuration

Full example: [`config.example.toml`](config.example.toml)

Key sections:
- `[agent]` — model, temperature, max tokens, system prompt
- `[[providers]]` — LLM provider definitions with failover priority
- `[safety]` — sandbox rules, allow-list, audit settings
- `[tools]` — enable/disable tool groups
- `[session]` — persistence and context window settings
- `[[ssh.hosts]]` — remote machine definitions
- `[[mcp.servers]]` — MCP server stubs (extensibility hook)

## Extensibility

- **Skills/plugins:** Load from `skills/` or `~/.forge/skills/` — each directory with a `SKILL.md` file
- **MCP client:** `[[mcp.servers]]` config section — tools auto-registered as `mcp_<server>_<tool>`
- **Hooks:** Pre/post tool and turn lifecycle via `HookRegistry` (see `docs/ARCHITECTURE.md`)
- **tree-sitter:** Code intelligence layer upgrade (planned)
- **Worktree isolation:** Parallel agents in git worktrees (`forge-parallel`)

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for full implementation status and roadmap.

## Development

```bash
cargo build
cargo test
RUST_LOG=forge=debug forge --trust exec "hello"
```

## License

MIT
