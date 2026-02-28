use super::{AgentProfile, InstallHint};

pub const PROFILE: AgentProfile = AgentProfile {
    name: "claude",
    binary: "claude",
    display_name: "Claude Code",
    install_hint: InstallHint::uniform("curl -fsSL https://claude.ai/install.sh | bash"),
};
