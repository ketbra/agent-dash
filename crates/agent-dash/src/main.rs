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

fn main() {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Commands::Status) => {
            println!("status: not yet implemented");
        }
        Some(cmd) => match cmd {
            Commands::Run { agent, args } => {
                println!("run: not yet implemented (agent={agent}, args={args:?})");
            }
            Commands::Daemon { action } => {
                println!("daemon: not yet implemented (action={action:?})");
            }
            Commands::Status => unreachable!(),
            Commands::Messages {
                session_id,
                format,
                limit,
            } => {
                println!(
                    "messages: not yet implemented (session={session_id}, format={format}, limit={limit:?})"
                );
            }
            Commands::Sessions { project } => {
                println!("sessions: not yet implemented (project={project})");
            }
            Commands::Watch {
                session_id,
                format,
            } => {
                println!(
                    "watch: not yet implemented (session={session_id}, format={format})"
                );
            }
            Commands::Inject { session_id, text } => {
                println!("inject: not yet implemented (session={session_id}, text={text})");
            }
            Commands::Hook { event_type } => {
                println!("hook: not yet implemented (event_type={event_type})");
            }
            Commands::Setup { target } => {
                println!("setup: not yet implemented (target={target:?})");
            }
            Commands::WatchEvents => {
                println!("watch-events: not yet implemented");
            }
            Commands::Approve { request_id } => {
                println!("approve: not yet implemented (request_id={request_id})");
            }
            Commands::ApproveSimilar { request_id } => {
                println!(
                    "approve-similar: not yet implemented (request_id={request_id})"
                );
            }
            Commands::Deny { request_id } => {
                println!("deny: not yet implemented (request_id={request_id})");
            }
        },
    }
}
