use agentdesk_rmm::redact_sensitive_text;
use base64::engine::general_purpose::{STANDARD as B64_STD, URL_SAFE_NO_PAD};
use base64::Engine;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiProviderKind {
    ChatGptOAuth,
    OpenAiApi,
    OllamaLocal,
    Disabled,
}

impl Default for AiProviderKind {
    fn default() -> Self {
        Self::ChatGptOAuth
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiAutonomyLevel {
    ConsultOnly,
    DiagnosticRead,
    ProposeActions,
    HumanApprovedExec,
}

impl Default for AiAutonomyLevel {
    fn default() -> Self {
        Self::HumanApprovedExec
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    pub provider: AiProviderKind,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub oauth_client_id: String,
    pub oauth_auth_url: String,
    pub oauth_token_url: String,
    pub oauth_access_token: String,
    pub oauth_refresh_token: String,
    pub autonomy_level: AiAutonomyLevel,
    pub redact_pii: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: AiProviderKind::ChatGptOAuth,
            model: "gpt-4o-mini".into(),
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            oauth_client_id: "agentdesk-desktop-client".into(),
            oauth_auth_url: "https://auth.openai.com/oauth/authorize".into(),
            oauth_token_url: "https://auth.openai.com/oauth/token".into(),
            oauth_access_token: String::new(),
            oauth_refresh_token: String::new(),
            autonomy_level: AiAutonomyLevel::HumanApprovedExec,
            redact_pii: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

pub fn generate_pkce() -> PkceChallenge {
    let mut rng = rand::thread_rng();
    let mut v_bytes = [0u8; 32];
    let mut s_bytes = [0u8; 16];
    rng.fill(&mut v_bytes);
    rng.fill(&mut s_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(v_bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = URL_SAFE_NO_PAD.encode(s_bytes);
    PkceChallenge {
        verifier,
        challenge,
        state,
    }
}

pub fn build_oauth_authorize_url(
    cfg: &AiConfig,
    redirect_uri: &str,
    pkce: &PkceChallenge,
) -> String {
    let auth_base = cfg.oauth_auth_url.trim_end_matches('?');
    let sep = if auth_base.contains('?') { '&' } else { '?' };
    format!(
        "{auth_base}{sep}response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        urlencoding::encode(&cfg.oauth_client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode("openid profile email offline_access model.read model.request"),
        urlencoding::encode(&pkce.challenge),
        urlencoding::encode(&pkce.state),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokenResult {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
}

pub async fn exchange_oauth_code(
    http: &reqwest::Client,
    cfg: &AiConfig,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> anyhow::Result<OAuthTokenResult> {
    let resp = http
        .post(&cfg.oauth_token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", cfg.oauth_client_id.as_str()),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("OAuth token exchange failed ({status}): {body}");
    }
    let val: serde_json::Value = resp.json().await?;
    let access_token = val
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing access_token in OAuth response"))?
        .to_string();
    let refresh_token = val
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let expires_in = val.get("expires_in").and_then(|v| v.as_u64());
    Ok(OAuthTokenResult {
        access_token,
        refresh_token,
        expires_in,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedAction {
    pub id: String,
    pub title: String,
    pub shell: String,
    pub command: String,
    pub risk_level: String,
    pub rationale: String,
}

#[derive(Debug, Clone)]
pub struct CopilotRequest {
    pub prompt: String,
    pub machine_context: Option<String>,
    pub screenshot_jpeg_b64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotResponse {
    pub reply: String,
    pub proposed_actions: Vec<ProposedAction>,
    pub provider_used: String,
    pub model_used: String,
    pub redacted_context_applied: bool,
}

const SYSTEM_PROMPT: &str = r#"Eres el Copiloto Técnico de AgentDesk para soporte remoto e infraestructura Windows/Linux.
Analiza los datos diagnósticos y/o captura de pantalla del equipo conectado y responde en español claro y conciso.
NUNCA inventes métricas ni afirmes que un comando ya se ejecutó.
Si necesitas sugerir comandos diagnósticos o correctivos para que el técnico apruebe en AgentDesk, incluye al final un bloque JSON exacto delimitado por:
```agentdesk-actions
[
  {"id":"act1","title":"Título corto","shell":"powershell","command":"Get-Service AgentDesk*","risk_level":"low","rationale":"Por qué ayuda"}
]
```"#;

pub fn parse_proposed_actions(reply: &str) -> Vec<ProposedAction> {
    let marker = "```agentdesk-actions";
    let Some(start) = reply.find(marker) else {
        return Vec::new();
    };
    let rest = &reply[start + marker.len()..];
    let Some(end) = rest.find("```") else {
        return Vec::new();
    };
    let json_str = rest[..end].trim();
    serde_json::from_str::<Vec<ProposedAction>>(json_str).unwrap_or_default()
}

pub async fn ask_copilot(
    http: &reqwest::Client,
    cfg: &AiConfig,
    req: CopilotRequest,
) -> anyhow::Result<CopilotResponse> {
    if cfg.provider == AiProviderKind::Disabled {
        anyhow::bail!("El módulo de IA está deshabilitado en Configuración IA");
    }

    let prompt_clean = if cfg.redact_pii {
        redact_sensitive_text(&req.prompt)
    } else {
        req.prompt.clone()
    };
    let ctx_clean = req.machine_context.map(|c| {
        if cfg.redact_pii {
            redact_sensitive_text(&c)
        } else {
            c
        }
    });

    let mut full_user_text = String::new();
    if let Some(ctx) = &ctx_clean {
        full_user_text.push_str("=== CONTEXTO DIAGNÓSTICO DE LA MÁQUINA ===\n");
        full_user_text.push_str(ctx);
        full_user_text.push_str("\n\n");
    }
    full_user_text.push_str("=== CONSULTA DEL TÉCNICO ===\n");
    full_user_text.push_str(&prompt_clean);

    match cfg.provider {
        AiProviderKind::ChatGptOAuth | AiProviderKind::OpenAiApi => {
            let token = if cfg.provider == AiProviderKind::ChatGptOAuth {
                if !cfg.oauth_access_token.trim().is_empty() {
                    cfg.oauth_access_token.trim()
                } else {
                    cfg.api_key.trim()
                }
            } else {
                cfg.api_key.trim()
            };

            if token.is_empty() {
                anyhow::bail!(
                    "Falta token ChatGPT OAuth o API Key de OpenAI. Conéctate con 'Conectar con ChatGPT (OAuth)' o configura Ollama local."
                );
            }

            let user_content = if let Some(b64) = &req.screenshot_jpeg_b64 {
                let clean_b64 = b64.trim();
                let _ = B64_STD.decode(clean_b64);
                serde_json::json!([
                    { "type": "text", "text": full_user_text },
                    { "type": "image_url", "image_url": { "url": format!("data:image/jpeg;base64,{clean_b64}") } }
                ])
            } else {
                serde_json::Value::String(full_user_text)
            };

            let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
            let resp = http
                .post(url)
                .bearer_auth(token)
                .json(&serde_json::json!({
                    "model": cfg.model,
                    "messages": [
                        { "role": "system", "content": SYSTEM_PROMPT },
                        { "role": "user", "content": user_content }
                    ],
                    "temperature": 0.2
                }))
                .send()
                .await?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("Error del proveedor IA ({status}): {body}");
            }

            let data: serde_json::Value = resp.json().await?;
            let reply = data
                .pointer("/choices/0/message/content")
                .and_then(|v| v.as_str())
                .unwrap_or("(sin respuesta del modelo)")
                .to_string();
            let proposed_actions = parse_proposed_actions(&reply);

            Ok(CopilotResponse {
                reply,
                proposed_actions,
                provider_used: if cfg.provider == AiProviderKind::ChatGptOAuth {
                    "chatgpt_oauth".into()
                } else {
                    "openai_api".into()
                },
                model_used: cfg.model.clone(),
                redacted_context_applied: cfg.redact_pii,
            })
        }
        AiProviderKind::OllamaLocal => {
            let base = if cfg.base_url.trim().is_empty() || cfg.base_url.contains("openai.com") {
                "http://127.0.0.1:11434"
            } else {
                cfg.base_url.trim_end_matches('/')
            };
            let model = if cfg.model.trim().is_empty() || cfg.model.starts_with("gpt-") {
                "llama3.2"
            } else {
                cfg.model.trim()
            };

            let mut msg = serde_json::json!({
                "role": "user",
                "content": full_user_text
            });
            if let Some(b64) = &req.screenshot_jpeg_b64 {
                msg["images"] = serde_json::json!([b64.trim()]);
            }

            let resp = http
                .post(format!("{base}/api/chat"))
                .json(&serde_json::json!({
                    "model": model,
                    "stream": false,
                    "messages": [
                        { "role": "system", "content": SYSTEM_PROMPT },
                        msg
                    ]
                }))
                .send()
                .await?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("Error Ollama ({status}): {body}");
            }

            let data: serde_json::Value = resp.json().await?;
            let reply = data
                .pointer("/message/content")
                .and_then(|v| v.as_str())
                .unwrap_or("(sin respuesta de Ollama)")
                .to_string();
            let proposed_actions = parse_proposed_actions(&reply);

            Ok(CopilotResponse {
                reply,
                proposed_actions,
                provider_used: "ollama_local".into(),
                model_used: model.to_string(),
                redacted_context_applied: cfg.redact_pii,
            })
        }
        AiProviderKind::Disabled => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_and_authorize_url_valid() {
        let pkce = generate_pkce();
        assert!(pkce.verifier.len() >= 40);
        assert!(pkce.challenge.len() >= 40);
        assert_ne!(pkce.verifier, pkce.challenge);
        let cfg = AiConfig::default();
        let url = build_oauth_authorize_url(&cfg, "http://127.0.0.1:8080/api/v1/ai/oauth/callback", &pkce);
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&pkce.state));
    }

    #[test]
    fn parses_agentdesk_actions_block() {
        let reply = "Te recomiendo revisar el servicio.\n```agentdesk-actions\n[{\"id\":\"a1\",\"title\":\"Ver servicios\",\"shell\":\"powershell\",\"command\":\"Get-Service AgentDesk*\",\"risk_level\":\"low\",\"rationale\":\"Diagnóstico\"}]\n```";
        let actions = parse_proposed_actions(reply);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].command, "Get-Service AgentDesk*");
    }
}
