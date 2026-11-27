use agentdesk_rmm::redact_sensitive_text;
use base64::engine::general_purpose::{STANDARD as B64_STD, URL_SAFE_NO_PAD};
use base64::Engine;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

pub const OPENCODE_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const OPENCODE_OAUTH_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const OPENCODE_OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const OPENCODE_OAUTH_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
pub const OPENCODE_DEVICE_USERCODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
pub const OPENCODE_DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
pub const OPENCODE_DEVICE_VERIFY_URL: &str = "https://auth.openai.com/codex/device";
pub const OPENCODE_DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
pub const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
pub const CODEX_MODELS_URL: &str =
    "https://chatgpt.com/backend-api/codex/models?client_version=0.172.0";

const CODEX_FALLBACK_MODELS: &[(&str, &str)] = &[
    ("gpt-6-luna", "GPT-6 Luna (ChatGPT OAuth)"),
    ("gpt-6-sol", "GPT-6 Sol (ChatGPT OAuth)"),
    ("gpt-5.6-luna", "GPT-5.6 Luna (ChatGPT OAuth)"),
    ("gpt-5.6-sol", "GPT-5.6 Sol (ChatGPT OAuth)"),
    ("gpt-5.4", "GPT-5.4 (ChatGPT OAuth)"),
    ("gpt-5.3-codex", "GPT-5.3 Codex (ChatGPT OAuth)"),
    ("gpt-5.2-codex", "GPT-5.2 Codex (ChatGPT OAuth)"),
    ("gpt-5.1-codex-mini", "GPT-5.1 Codex Mini (ChatGPT OAuth)"),
];

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
    pub reasoning_effort: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: AiProviderKind::ChatGptOAuth,
            model: "gpt-6-luna".into(),
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            oauth_client_id: OPENCODE_OAUTH_CLIENT_ID.into(),
            oauth_auth_url: OPENCODE_OAUTH_AUTH_URL.into(),
            oauth_token_url: OPENCODE_OAUTH_TOKEN_URL.into(),
            oauth_access_token: String::new(),
            oauth_refresh_token: String::new(),
            autonomy_level: AiAutonomyLevel::HumanApprovedExec,
            redact_pii: true,
            reasoning_effort: "high".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiModelInfo {
    pub id: String,
    pub label: String,
    pub provider: String,
    #[serde(default)]
    pub reasoning_efforts: Vec<String>,
}

pub fn normalize_reasoning_effort(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "low" => "low".into(),
        "medium" => "medium".into(),
        "high" => "high".into(),
        "xhigh" => "xhigh".into(),
        _ => "high".into(),
    }
}

pub fn is_internal_codex_slug(slug: &str) -> bool {
    let s = slug.trim().to_ascii_lowercase();
    s.is_empty()
        || s.contains("auto-review")
        || s.ends_with("-review")
        || s.contains("internal")
        || s.contains("eval")
}

pub fn is_legacy_or_internal_model(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    m.is_empty() || m.starts_with("gpt-4o") || is_internal_codex_slug(&m)
}

fn parse_version_tuple(slug: &str) -> (u32, u32, u32) {
    let digits: Vec<u32> = slug
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse::<u32>().ok())
        .take(3)
        .collect();
    (
        digits.first().copied().unwrap_or(0),
        digits.get(1).copied().unwrap_or(0),
        digits.get(2).copied().unwrap_or(0),
    )
}

pub fn pick_default_luna_model(models: &[AiModelInfo]) -> Option<String> {
    if let Some(best) = models
        .iter()
        .filter(|m| m.id.to_ascii_lowercase().contains("luna") && !is_internal_codex_slug(&m.id))
        .max_by_key(|m| parse_version_tuple(&m.id))
    {
        return Some(best.id.clone());
    }
    models
        .iter()
        .find(|m| !is_internal_codex_slug(&m.id))
        .map(|m| m.id.clone())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
    pub nonce: String,
}

pub fn generate_pkce() -> PkceChallenge {
    let mut rng = rand::thread_rng();
    let mut v_bytes = [0u8; 32];
    let mut s_bytes = [0u8; 16];
    let mut n_bytes = [0u8; 24];
    rng.fill(&mut v_bytes);
    rng.fill(&mut s_bytes);
    rng.fill(&mut n_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(v_bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = URL_SAFE_NO_PAD.encode(s_bytes);
    let nonce = URL_SAFE_NO_PAD.encode(n_bytes);
    PkceChallenge {
        verifier,
        challenge,
        state,
        nonce,
    }
}

/// Builds the exact OpenCode-compatible ChatGPT PKCE authorize URL.
pub fn build_oauth_authorize_url(
    cfg: &AiConfig,
    redirect_uri: &str,
    pkce: &PkceChallenge,
) -> String {
    let auth_base = cfg.oauth_auth_url.trim_end_matches('?');
    let sep = if auth_base.contains('?') { '&' } else { '?' };
    let client_id = if cfg.oauth_client_id.trim().is_empty()
        || cfg.oauth_client_id == "dynamic_agent_client"
    {
        OPENCODE_OAUTH_CLIENT_ID
    } else {
        cfg.oauth_client_id.trim()
    };
    format!(
        "{auth_base}{sep}response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&id_token_add_organizations=true&codex_cli_simplified_flow=true&state={}&originator=opencode",
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode("openid profile email offline_access"),
        urlencoding::encode(&pkce.challenge),
        urlencoding::encode(&pkce.state),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokenResult {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expires_in: Option<u64>,
}

fn decode_jwt_claims(jwt: &str) -> Option<serde_json::Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim())
        .or_else(|_| B64_STD.decode(payload.trim()))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn extract_chatgpt_account_id(jwt: &str) -> Option<String> {
    let claims = decode_jwt_claims(jwt)?;
    if let Some(acct) = claims
        .get("https://api.openai.com/auth")
        .and_then(|v| v.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        return Some(acct.to_string());
    }
    if let Some(acct) = claims
        .get("chatgpt_account_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        return Some(acct.to_string());
    }
    claims
        .get("organizations")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|org| org.get("id"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

pub fn jwt_is_expired_or_soon(jwt: &str) -> bool {
    let Some(claims) = decode_jwt_claims(jwt) else {
        return false;
    };
    let Some(exp) = claims.get("exp").and_then(|v| v.as_u64()) else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    now + 60 >= exp
}

fn resolve_oauth_client_id<'a>(cfg: &'a AiConfig, override_cid: Option<&'a str>) -> &'a str {
    override_cid
        .filter(|v| v.starts_with("oaiapp_") || v.starts_with("app_"))
        .unwrap_or_else(|| {
            if cfg.oauth_client_id.trim().is_empty()
                || cfg.oauth_client_id == "dynamic_agent_client"
            {
                OPENCODE_OAUTH_CLIENT_ID
            } else {
                cfg.oauth_client_id.trim()
            }
        })
}

pub async fn exchange_oauth_code(
    http: &reqwest::Client,
    cfg: &AiConfig,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    callback_client_id: Option<&str>,
) -> anyhow::Result<OAuthTokenResult> {
    let client_id = resolve_oauth_client_id(cfg, callback_client_id);
    let resp = http
        .post(&cfg.oauth_token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
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
        .map(str::to_string);
    let id_token = val
        .get("id_token")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let expires_in = val.get("expires_in").and_then(|v| v.as_u64());
    Ok(OAuthTokenResult {
        access_token,
        refresh_token,
        id_token,
        expires_in,
    })
}

pub async fn refresh_oauth_token(
    http: &reqwest::Client,
    cfg: &AiConfig,
) -> anyhow::Result<OAuthTokenResult> {
    let rt = cfg.oauth_refresh_token.trim();
    if rt.is_empty() {
        anyhow::bail!("missing oauth_refresh_token");
    }
    let client_id = resolve_oauth_client_id(cfg, None);
    let resp = http
        .post(&cfg.oauth_token_url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", rt),
            ("client_id", client_id),
        ])
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("OAuth token refresh failed ({status}): {body}");
    }
    let val: serde_json::Value = resp.json().await?;
    let access_token = val
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing access_token in refresh response"))?
        .to_string();
    let refresh_token = val
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let id_token = val
        .get("id_token")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let expires_in = val.get("expires_in").and_then(|v| v.as_u64());
    Ok(OAuthTokenResult {
        access_token,
        refresh_token,
        id_token,
        expires_in,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceAuthStart {
    pub device_auth_id: String,
    pub user_code: String,
    pub interval_secs: u64,
    pub verify_url: String,
}

#[derive(Debug, Clone)]
pub enum DeviceAuthPoll {
    Pending,
    Authorized {
        authorization_code: String,
        code_verifier: String,
    },
    Failed(String),
}

fn parse_device_interval(val: &serde_json::Value) -> u64 {
    val.get("interval")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
        })
        .unwrap_or(5)
        .max(5)
}

pub async fn start_device_auth(
    http: &reqwest::Client,
    cfg: &AiConfig,
) -> anyhow::Result<DeviceAuthStart> {
    let client_id = resolve_oauth_client_id(cfg, None);
    let resp = http
        .post(OPENCODE_DEVICE_USERCODE_URL)
        .json(&serde_json::json!({ "client_id": client_id }))
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("OpenAI device auth start failed ({status}): {body}");
    }
    let val: serde_json::Value = resp.json().await?;
    let device_auth_id = val
        .get("device_auth_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing device_auth_id"))?
        .to_string();
    let user_code = val
        .get("user_code")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing user_code"))?
        .to_string();
    Ok(DeviceAuthStart {
        device_auth_id,
        user_code,
        interval_secs: parse_device_interval(&val),
        verify_url: OPENCODE_DEVICE_VERIFY_URL.to_string(),
    })
}

pub async fn poll_device_auth(
    http: &reqwest::Client,
    device_auth_id: &str,
    user_code: &str,
) -> anyhow::Result<DeviceAuthPoll> {
    let resp = http
        .post(OPENCODE_DEVICE_TOKEN_URL)
        .json(&serde_json::json!({
            "device_auth_id": device_auth_id,
            "user_code": user_code,
        }))
        .send()
        .await?;
    let status = resp.status();
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND {
        return Ok(DeviceAuthPoll::Pending);
    }
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Ok(DeviceAuthPoll::Failed(format!("({status}): {body}")));
    }
    let val: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| anyhow::anyhow!("invalid deviceauth token JSON: {e}"))?;
    let authorization_code = val
        .get("authorization_code")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing authorization_code"))?
        .to_string();
    let code_verifier = val
        .get("code_verifier")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing code_verifier"))?
        .to_string();
    Ok(DeviceAuthPoll::Authorized {
        authorization_code,
        code_verifier,
    })
}

pub async fn exchange_device_auth_code(
    http: &reqwest::Client,
    cfg: &AiConfig,
    authorization_code: &str,
    code_verifier: &str,
) -> anyhow::Result<OAuthTokenResult> {
    exchange_oauth_code(
        http,
        cfg,
        authorization_code,
        code_verifier,
        OPENCODE_DEVICE_REDIRECT_URI,
        None,
    )
    .await
}

pub async fn list_available_models(
    http: &reqwest::Client,
    cfg: &AiConfig,
) -> Vec<AiModelInfo> {
    match cfg.provider {
        AiProviderKind::Disabled => Vec::new(),
        AiProviderKind::ChatGptOAuth => {
            let tok = cfg.oauth_access_token.trim();
            if tok.is_empty() {
                return Vec::new();
            }
            let mut req = http
                .get(CODEX_MODELS_URL)
                .bearer_auth(tok)
                .header("originator", "opencode")
                .header("Accept", "application/json");
            if let Some(acct) = extract_chatgpt_account_id(tok) {
                req = req.header("chatgpt-account-id", acct);
            }
            if let Ok(resp) = req.send().await {
                if resp.status().is_success() {
                    if let Ok(val) = resp.json::<serde_json::Value>().await {
                        let arr = val
                            .get("models")
                            .and_then(|m| m.as_array())
                            .or_else(|| val.get("data").and_then(|m| m.as_array()));
                        if let Some(items) = arr {
                            let mut seen = BTreeSet::new();
                            let mut out = Vec::new();
                            for item in items {
                                let id = item
                                    .get("slug")
                                    .or_else(|| item.get("id"))
                                    .and_then(|v| v.as_str())
                                    .map(str::trim)
                                    .filter(|s| !is_internal_codex_slug(s));
                                if let Some(id) = id {
                                    if seen.insert(id.to_string()) {
                                        let title = item
                                            .get("display_name")
                                            .or_else(|| item.get("title"))
                                            .and_then(|v| v.as_str())
                                            .map(|t| format!("{t} ({id})"))
                                            .unwrap_or_else(|| id.to_string());
                                        let mut efforts = item
                                            .get("supported_reasoning_levels")
                                            .and_then(|v| v.as_array())
                                            .map(|lvls| {
                                                lvls.iter()
                                                    .filter_map(|x| {
                                                        x.get("effort")
                                                            .and_then(|e| e.as_str())
                                                            .or_else(|| x.as_str())
                                                            .map(normalize_reasoning_effort)
                                                    })
                                                    .collect::<Vec<_>>()
                                            })
                                            .unwrap_or_default();
                                        efforts.dedup();
                                        if efforts.is_empty() {
                                            efforts = vec![
                                                "low".into(),
                                                "medium".into(),
                                                "high".into(),
                                                "xhigh".into(),
                                            ];
                                        }
                                        out.push(AiModelInfo {
                                            id: id.to_string(),
                                            label: title,
                                            provider: "chat_gpt_o_auth".into(),
                                            reasoning_efforts: efforts,
                                        });
                                    }
                                }
                            }
                            if !out.is_empty() {
                                return out;
                            }
                        }
                    }
                }
            }
            CODEX_FALLBACK_MODELS
                .iter()
                .map(|(id, label)| AiModelInfo {
                    id: (*id).into(),
                    label: (*label).into(),
                    provider: "chat_gpt_o_auth".into(),
                    reasoning_efforts: vec![
                        "low".into(),
                        "medium".into(),
                        "high".into(),
                        "xhigh".into(),
                    ],
                })
                .collect()
        }
        AiProviderKind::OpenAiApi => {
            let key = cfg.api_key.trim();
            if key.is_empty() {
                return Vec::new();
            }
            let url = format!("{}/models", cfg.base_url.trim_end_matches('/'));
            if let Ok(resp) = http.get(url).bearer_auth(key).send().await {
                if resp.status().is_success() {
                    if let Ok(val) = resp.json::<serde_json::Value>().await {
                        if let Some(items) = val.get("data").and_then(|d| d.as_array()) {
                            let mut ids: Vec<String> = items
                                .iter()
                                .filter_map(|i| i.get("id")?.as_str())
                                .filter(|id| {
                                    id.starts_with("gpt-")
                                        || id.starts_with("o1")
                                        || id.starts_with("o3")
                                        || id.starts_with("o4")
                                        || id.starts_with("chatgpt-")
                                })
                                .map(str::to_string)
                                .collect();
                            ids.sort();
                            ids.dedup();
                            if !ids.is_empty() {
                                return ids
                                    .into_iter()
                                    .map(|id| AiModelInfo {
                                        label: id.clone(),
                                        id,
                                        provider: "openai_api".into(),
                                        reasoning_efforts: vec![
                                            "low".into(),
                                            "medium".into(),
                                            "high".into(),
                                        ],
                                    })
                                    .collect();
                            }
                        }
                    }
                }
            }
            vec![
                AiModelInfo { id: "gpt-4o-mini".into(), label: "gpt-4o-mini".into(), provider: "openai_api".into(), reasoning_efforts: vec!["low".into(), "medium".into(), "high".into()] },
                AiModelInfo { id: "gpt-4o".into(), label: "gpt-4o".into(), provider: "openai_api".into(), reasoning_efforts: vec!["low".into(), "medium".into(), "high".into()] },
                AiModelInfo { id: "gpt-4.1".into(), label: "gpt-4.1".into(), provider: "openai_api".into(), reasoning_efforts: vec!["low".into(), "medium".into(), "high".into()] },
                AiModelInfo { id: "o4-mini".into(), label: "o4-mini".into(), provider: "openai_api".into(), reasoning_efforts: vec!["low".into(), "medium".into(), "high".into()] },
            ]
        }
        AiProviderKind::OllamaLocal => {
            let base = if cfg.base_url.trim().is_empty() || cfg.base_url.contains("openai.com") {
                "http://127.0.0.1:11434"
            } else {
                cfg.base_url.trim_end_matches('/')
            };
            if let Ok(resp) = http.get(format!("{base}/api/tags")).send().await {
                if resp.status().is_success() {
                    if let Ok(val) = resp.json::<serde_json::Value>().await {
                        if let Some(models) = val.get("models").and_then(|m| m.as_array()) {
                            let out: Vec<AiModelInfo> = models
                                .iter()
                                .filter_map(|m| m.get("name")?.as_str())
                                .map(|name| AiModelInfo {
                                    id: name.to_string(),
                                    label: name.to_string(),
                                    provider: "ollama_local".into(),
                                    reasoning_efforts: Vec::new(),
                                })
                                .collect();
                            if !out.is_empty() {
                                return out;
                            }
                        }
                    }
                }
            }
            Vec::new()
        }
    }
}

pub fn parse_codex_sse_reply(body: &str) -> String {
    let mut deltas = String::new();
    let mut completed_text = String::new();

    for raw_line in body.lines() {
        let line = raw_line.trim();
        let Some(payload) = line.strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<serde_json::Value>(payload) else {
            continue;
        };
        match ev.get("type").and_then(|v| v.as_str()) {
            Some("response.output_text.delta") => {
                if let Some(delta) = ev.get("delta").and_then(|v| v.as_str()) {
                    deltas.push_str(delta);
                }
            }
            Some("response.output_text.done") => {
                if deltas.is_empty() {
                    if let Some(text) = ev.get("text").and_then(|v| v.as_str()) {
                        completed_text.push_str(text);
                    }
                }
            }
            Some("response.completed") => {
                if deltas.is_empty() && completed_text.is_empty() {
                    if let Some(outputs) = ev
                        .get("response")
                        .and_then(|r| r.get("output"))
                        .and_then(|o| o.as_array())
                    {
                        for item in outputs {
                            if let Some(parts) = item.get("content").and_then(|c| c.as_array()) {
                                for part in parts {
                                    if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                                        completed_text.push_str(txt);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if !deltas.is_empty() {
        deltas
    } else if !completed_text.is_empty() {
        completed_text
    } else {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| {
                v.get("output_text")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
                    .or_else(|| {
                        v.pointer("/output/0/content/0/text")
                            .and_then(|x| x.as_str())
                            .map(str::to_string)
                    })
            })
            .unwrap_or_default()
    }
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
pub struct ChatHistoryTurn {
    pub prompt: String,
    pub reply: String,
}

#[derive(Debug, Clone)]
pub struct CopilotRequest {
    pub prompt: String,
    pub machine_context: Option<String>,
    pub screenshot_jpeg_b64: Option<String>,
    pub model_override: Option<String>,
    pub reasoning_effort_override: Option<String>,
    pub history: Vec<ChatHistoryTurn>,
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
        AiProviderKind::ChatGptOAuth => {
            let token = cfg.oauth_access_token.trim();
            if token.is_empty() {
                anyhow::bail!(
                    "Falta vincular ChatGPT OAuth. Usá 'Vincular ChatGPT (OAuth)' en AgentDesk AI."
                );
            }
            let model_raw = req
                .model_override
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| cfg.model.trim());
            let model = if is_legacy_or_internal_model(model_raw) {
                "gpt-6-luna"
            } else {
                model_raw
            };
            let effort = normalize_reasoning_effort(
                req.reasoning_effort_override
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or(&cfg.reasoning_effort),
            );
            let mut input_items: Vec<serde_json::Value> = Vec::new();
            for turn in &req.history {
                input_items.push(serde_json::json!({
                    "role": "user",
                    "content": [{ "type": "input_text", "text": turn.prompt }]
                }));
                input_items.push(serde_json::json!({
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": turn.reply }]
                }));
            }
            let mut content_parts = vec![serde_json::json!({
                "type": "input_text",
                "text": full_user_text
            })];
            if let Some(b64) = &req.screenshot_jpeg_b64 {
                let clean_b64 = b64.trim();
                let _ = B64_STD.decode(clean_b64);
                content_parts.push(serde_json::json!({
                    "type": "input_image",
                    "image_url": format!("data:image/jpeg;base64,{clean_b64}")
                }));
            }
            input_items.push(serde_json::json!({
                "role": "user",
                "content": content_parts
            }));

            let mut rb = http
                .post(CODEX_RESPONSES_URL)
                .bearer_auth(token)
                .header("originator", "opencode")
                .header("OpenAI-Beta", "responses=experimental")
                .header("Accept", "text/event-stream");
            if let Some(acct) = extract_chatgpt_account_id(token) {
                rb = rb.header("chatgpt-account-id", acct);
            }

            let resp = rb
                .json(&serde_json::json!({
                    "model": model,
                    "instructions": SYSTEM_PROMPT,
                    "input": input_items,
                    "reasoning": { "effort": effort },
                    "store": false,
                    "stream": true
                }))
                .send()
                .await?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("Error ChatGPT OAuth Codex ({status}): {body}");
            }

            let body = resp.text().await?;
            let reply = parse_codex_sse_reply(&body);
            let reply = if reply.is_empty() {
                "(sin respuesta del modelo Codex)".to_string()
            } else {
                reply
            };
            let proposed_actions = parse_proposed_actions(&reply);

            Ok(CopilotResponse {
                reply,
                proposed_actions,
                provider_used: "chatgpt_oauth".into(),
                model_used: model.to_string(),
                redacted_context_applied: cfg.redact_pii,
            })
        }
        AiProviderKind::OpenAiApi => {
            let token = cfg.api_key.trim();
            if token.is_empty() {
                anyhow::bail!("Falta API Key de OpenAI API.");
            }
            let model = req
                .model_override
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| cfg.model.trim());
            let model = if model.is_empty() { "gpt-4o-mini" } else { model };

            let mut messages = vec![serde_json::json!({ "role": "system", "content": SYSTEM_PROMPT })];
            for turn in &req.history {
                messages.push(serde_json::json!({ "role": "user", "content": turn.prompt }));
                messages.push(serde_json::json!({ "role": "assistant", "content": turn.reply }));
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
            messages.push(serde_json::json!({ "role": "user", "content": user_content }));

            let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
            let resp = http
                .post(url)
                .bearer_auth(token)
                .json(&serde_json::json!({
                    "model": model,
                    "messages": messages,
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
                provider_used: "openai_api".into(),
                model_used: model.to_string(),
                redacted_context_applied: cfg.redact_pii,
            })
        }
        AiProviderKind::OllamaLocal => {
            let base = if cfg.base_url.trim().is_empty() || cfg.base_url.contains("openai.com") {
                "http://127.0.0.1:11434"
            } else {
                cfg.base_url.trim_end_matches('/')
            };
            let raw_model = req
                .model_override
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| cfg.model.trim());
            let model = if raw_model.is_empty() || raw_model.starts_with("gpt-") {
                "llama3.2"
            } else {
                raw_model
            };

            let mut messages = vec![serde_json::json!({ "role": "system", "content": SYSTEM_PROMPT })];
            for turn in &req.history {
                messages.push(serde_json::json!({ "role": "user", "content": turn.prompt }));
                messages.push(serde_json::json!({ "role": "assistant", "content": turn.reply }));
            }
            let mut msg = serde_json::json!({
                "role": "user",
                "content": full_user_text
            });
            if let Some(b64) = &req.screenshot_jpeg_b64 {
                msg["images"] = serde_json::json!([b64.trim()]);
            }
            messages.push(msg);

            let resp = http
                .post(format!("{base}/api/chat"))
                .json(&serde_json::json!({
                    "model": model,
                    "stream": false,
                    "messages": messages
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
        let url = build_oauth_authorize_url(&cfg, OPENCODE_OAUTH_REDIRECT_URI, &pkce);
        assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"));
        assert!(url.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("codex_cli_simplified_flow=true"));
        assert!(url.contains("originator=opencode"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
        assert!(url.contains(&pkce.state));
    }

    #[test]
    fn extracts_chatgpt_account_id_from_jwt() {
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"RS256\"}");
        let payload = URL_SAFE_NO_PAD.encode(
            br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_test_123"},"exp":4102444800}"#,
        );
        let jwt = format!("{header}.{payload}.sig");
        assert_eq!(extract_chatgpt_account_id(&jwt).as_deref(), Some("acct_test_123"));
        assert!(!jwt_is_expired_or_soon(&jwt));
    }

    #[test]
    fn parses_codex_sse_stream() {
        let sse = "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hola \"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"AgentDesk\"}\n\ndata: [DONE]\n";
        assert_eq!(parse_codex_sse_reply(sse), "Hola AgentDesk");
    }

    #[test]
    fn parses_agentdesk_actions_block() {
        let reply = "Te recomiendo revisar el servicio.\n```agentdesk-actions\n[{\"id\":\"a1\",\"title\":\"Ver servicios\",\"shell\":\"powershell\",\"command\":\"Get-Service AgentDesk*\",\"risk_level\":\"low\",\"rationale\":\"Diagnóstico\"}]\n```";
        let actions = parse_proposed_actions(reply);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].command, "Get-Service AgentDesk*");
    }

    #[test]
    fn device_auth_constants_and_interval_parsing() {
        assert_eq!(
            OPENCODE_DEVICE_USERCODE_URL,
            "https://auth.openai.com/api/accounts/deviceauth/usercode"
        );
        assert_eq!(
            OPENCODE_DEVICE_TOKEN_URL,
            "https://auth.openai.com/api/accounts/deviceauth/token"
        );
        assert_eq!(OPENCODE_DEVICE_VERIFY_URL, "https://auth.openai.com/codex/device");
        assert_eq!(
            OPENCODE_DEVICE_REDIRECT_URI,
            "https://auth.openai.com/deviceauth/callback"
        );
        assert_eq!(parse_device_interval(&serde_json::json!({"interval": "7"})), 7);
        assert_eq!(parse_device_interval(&serde_json::json!({"interval": 2})), 5);
        assert_eq!(parse_device_interval(&serde_json::json!({"interval": 12})), 12);
    }

    #[test]
    fn picks_latest_luna_model_and_filters_internal_slugs() {
        assert!(is_internal_codex_slug("codex-auto-review"));
        assert!(is_legacy_or_internal_model("codex-auto-review"));
        assert!(is_legacy_or_internal_model("gpt-4o-mini"));
        assert!(!is_legacy_or_internal_model("gpt-6-luna"));
        let models = vec![
            AiModelInfo { id: "gpt-5.6-luna".into(), label: "5.6".into(), provider: "chat_gpt_o_auth".into(), reasoning_efforts: vec![] },
            AiModelInfo { id: "gpt-6-sol".into(), label: "6 Sol".into(), provider: "chat_gpt_o_auth".into(), reasoning_efforts: vec![] },
            AiModelInfo { id: "gpt-6.1-luna".into(), label: "6.1 Luna".into(), provider: "chat_gpt_o_auth".into(), reasoning_efforts: vec![] },
            AiModelInfo { id: "gpt-6-luna".into(), label: "6 Luna".into(), provider: "chat_gpt_o_auth".into(), reasoning_efforts: vec![] },
        ];
        assert_eq!(pick_default_luna_model(&models).as_deref(), Some("gpt-6.1-luna"));
    }

    #[test]
    fn normalizes_reasoning_effort() {
        assert_eq!(normalize_reasoning_effort("HIGH"), "high");
        assert_eq!(normalize_reasoning_effort("xhigh"), "xhigh");
        assert_eq!(normalize_reasoning_effort("low"), "low");
        assert_eq!(normalize_reasoning_effort("unknown"), "high");
    }
}
