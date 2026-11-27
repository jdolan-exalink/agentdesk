include!("../../build-support/winres.rs");

fn main() {
    embed_windows_resources(
        "../../assets/icons/agentdesk-client.ico",
        "agentdesk-host",
        "AgentDesk Host - agente de soporte remoto",
        true,
    );
}
