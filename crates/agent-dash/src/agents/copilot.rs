use super::{AgentProfile, InstallHint};

pub const PROFILE: AgentProfile = AgentProfile {
    name: "copilot",
    binary: "copilot",
    display_name: "GitHub Copilot",
    install_hint: InstallHint {
        linux: "curl -fsSL https://gh.io/copilot-install | bash",
        macos: "brew install copilot-cli",
        windows: "winget install GitHub.Copilot",
    },
};
