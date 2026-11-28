include!("../../build-support/winres.rs");

fn main() {
    embed_windows_resources(
        "../../assets/icons/agentdesk-server.ico",
        "agentdesk-server",
        "AgentDesk Server - servidor de soporte remoto",
        true,
    );
}
