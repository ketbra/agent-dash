use super::{AgentProfile, InstallHint};

pub const PROFILE: AgentProfile = AgentProfile {
    name: "claude",
    binary: "claude",
    display_name: "Claude Code",
    install_hint: InstallHint {
        linux: "curl -fsSL https://claude.ai/install.sh | bash",
        macos: "curl -fsSL https://claude.ai/install.sh | bash",
        windows: "powershell -c \"irm https://claude.ai/install.ps1 | iex\"",
    },
};
