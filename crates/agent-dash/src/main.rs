mod agents;
mod cli;
mod client_listener;
mod daemon;
mod hook_cmd;
mod hook_listener;
mod messages;
mod scanner;
mod setup;
mod state;
mod watcher;
mod wrapper;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agent-dash", about = "Unified CLI for agent-dash")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Wrap and run an agent in a PTY
    Run {
        /// Agent to run (e.g. "claude")
        agent: String,
        /// Arguments to pass to the agent
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Manage the daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },

    /// Show all active sessions
    Status,

    /// Fetch messages from a session
    Messages {
        /// Session ID
        session_id: String,
        /// Output format (e.g. "json", "text")
        #[arg(default_value = "text")]
        format: String,
        /// Maximum number of messages to return
        #[arg(short, long)]
        limit: Option<usize>,
    },

    /// List JSONL sessions for a project
    Sessions {
        /// Project path or name
        project: String,
    },

    /// Stream new messages from a session
    Watch {
        /// Session ID
        session_id: String,
        /// Output format (e.g. "json", "text")
        #[arg(default_value = "text")]
        format: String,
    },

    /// Inject a prompt into a wrapped session
    Inject {
        /// Session ID
        session_id: String,
        /// Text to inject
        text: String,
    },

    /// Handle a Claude Code hook event
    Hook {
        /// Hook event type (e.g. "PreToolUse", "PostToolUse")
        event_type: String,
    },

    /// Install hooks and check dependencies
    Setup {
        /// Target to set up (e.g. "hooks", "all")
        target: Option<String>,
    },

    /// Subscribe to raw daemon event stream
    WatchEvents,

    /// Approve a permission request
    Approve {
        /// Request ID to approve
        request_id: String,
    },

    /// Approve similar permission requests
    ApproveSimilar {
        /// Request ID whose pattern to approve
        request_id: String,
    },

    /// Deny a permission request
    Deny {
        /// Request ID to deny
        request_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum DaemonAction {
    /// Start the daemon
    Start,
    /// Stop the daemon
    Stop,
    /// Show daemon status
    Status,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Run { agent, args }) => {
            let profile = agents::lookup(&agent).unwrap_or_else(|| {
                eprintln!("Unknown agent: {agent}");
                eprintln!("Supported agents: claude");
                std::process::exit(1);
            });
            let exit_code = wrapper::run(profile, &args);
            std::process::exit(exit_code);
        }
        Some(Commands::Daemon { action }) => match action {
            DaemonAction::Start => {
                daemon::run().await;
            }
            DaemonAction::Stop => println!("daemon stop: not yet implemented"),
            DaemonAction::Status => println!("daemon status: not yet implemented"),
        },
        Some(Commands::Status) | None => cli::cmd_status(),
        Some(Commands::Messages { session_id, format, limit }) => {
            cli::cmd_messages(&session_id, &format, limit.unwrap_or(20));
        }
        Some(Commands::Sessions { project }) => cli::cmd_sessions(&project),
        Some(Commands::Watch { session_id, format }) => {
            cli::cmd_watch_messages(&session_id, &format);
        }
        Some(Commands::WatchEvents) => cli::cmd_watch(),
        Some(Commands::Inject { session_id, text }) => {
            cli::cmd_inject(&session_id, &text);
        }
        Some(Commands::Hook { event_type }) => hook_cmd::run(&event_type),
        Some(Commands::Setup { target }) => {
            let target = target.as_deref().unwrap_or("all");
            match target {
                "hooks" | "all" => match setup::install_hooks(false) {
                    Ok(true) => println!("Hooks installed successfully."),
                    Ok(false) => println!("Hooks already up to date."),
                    Err(e) => {
                        eprintln!("Failed to install hooks: {e}");
                        std::process::exit(1);
                    }
                },
                _ => {
                    eprintln!("Unknown setup target: {target}");
                    eprintln!("Available: hooks, all");
                    std::process::exit(1);
                }
            }
        }
        Some(Commands::Approve { request_id }) => {
            cli::cmd_permission_response(&request_id, "allow");
        }
        Some(Commands::ApproveSimilar { request_id }) => {
            cli::cmd_permission_response(&request_id, "allow_similar");
        }
        Some(Commands::Deny { request_id }) => {
            cli::cmd_permission_response(&request_id, "deny");
        }
    }
}
