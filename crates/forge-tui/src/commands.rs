#[derive(Debug)]
pub enum SlashCommand {
    Help,
    Model(Option<String>),
    Tools,
    Clear,
    Debug,
    Parallel(Vec<String>),
    Ssh(Vec<String>),
    Resume,
    Skills,
    Quit,
    ToggleMouse,
    Unknown(String),
}

impl SlashCommand {
    pub fn parse(input: &str) -> Self {
        let input = input.trim();
        if !input.starts_with('/') {
            return SlashCommand::Unknown(input.to_string());
        }

        let parts: Vec<&str> = input[1..].split_whitespace().collect();
        if parts.is_empty() {
            return SlashCommand::Help;
        }

        match parts[0] {
            "help" => SlashCommand::Help,
            "model" => SlashCommand::Model(parts.get(1).map(|s| s.to_string())),
            "tools" => SlashCommand::Tools,
            "clear" => SlashCommand::Clear,
            "debug" => SlashCommand::Debug,
            "parallel" => {
                let tasks: Vec<String> = if parts.len() > 1 {
                    parts[1..]
                        .join(" ")
                        .split(';')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                } else {
                    vec![]
                };
                SlashCommand::Parallel(tasks)
            }
            "ssh" => SlashCommand::Ssh(parts[1..].iter().map(|s| s.to_string()).collect()),
            "resume" => SlashCommand::Resume,
            "skills" => SlashCommand::Skills,
            "toggle-mouse" | "mouse" => SlashCommand::ToggleMouse,
            "quit" | "exit" => SlashCommand::Quit,
            other => SlashCommand::Unknown(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_command() {
        match SlashCommand::parse("/model gpt-4o") {
            SlashCommand::Model(Some(m)) => assert_eq!(m, "gpt-4o"),
            _ => panic!("expected model command"),
        }
    }

    #[test]
    fn parses_parallel_tasks() {
        match SlashCommand::parse("/parallel fix tests; update docs") {
            SlashCommand::Parallel(tasks) => {
                assert_eq!(tasks.len(), 2);
            }
            _ => panic!("expected parallel command"),
        }
    }

    #[test]
    fn parses_exit_command() {
        assert!(matches!(
            SlashCommand::parse("/exit"),
            SlashCommand::Quit
        ));
        assert!(matches!(
            SlashCommand::parse("/quit"),
            SlashCommand::Quit
        ));
    }
}
