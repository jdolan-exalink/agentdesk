include!("../../build-support/winres.rs");

fn main() {
    embed_windows_resources(
        "../../assets/icons/agentdesk-server.ico",
        "agentdesk-server-console",
        "AgentDesk Server Console",
        false,
    );
}
