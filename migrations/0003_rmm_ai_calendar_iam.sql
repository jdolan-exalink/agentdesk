-- Phase 3/4: RMM Telemetry & Terminal, AI Copilot (ChatGPT OAuth), Calendar, Users MFA, Ticket Comments, Public Docs & Branding

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS mfa_secret TEXT NULL,
    ADD COLUMN IF NOT EXISTS mfa_enabled BOOLEAN NOT NULL DEFAULT false;

ALTER TABLE organizations
    ADD COLUMN IF NOT EXISTS brand_logo_url TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS brand_tagline TEXT NOT NULL DEFAULT 'Soporte remoto inteligente y self-hosted';

CREATE TABLE IF NOT EXISTS device_telemetry (
    id BIGSERIAL PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    payload JSONB NOT NULL,
    collected_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS rmm_commands (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    requested_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    shell TEXT NOT NULL,
    command TEXT NOT NULL,
    exit_code INTEGER NOT NULL DEFAULT 0,
    stdout TEXT NOT NULL DEFAULT '',
    stderr TEXT NOT NULL DEFAULT '',
    duration_ms BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS ticket_comments (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    ticket_id UUID NOT NULL REFERENCES support_requests(id) ON DELETE CASCADE,
    author_id UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    author_name TEXT NOT NULL,
    is_internal BOOLEAN NOT NULL DEFAULT false,
    body TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS calendar_events (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    technician_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id UUID NULL REFERENCES devices(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    client_name TEXT NOT NULL DEFAULT '',
    starts_at TIMESTAMPTZ NOT NULL,
    ends_at TIMESTAMPTZ NOT NULL,
    notes TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS ai_settings (
    organization_id UUID PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
    provider TEXT NOT NULL DEFAULT 'chat_gpt_o_auth',
    model TEXT NOT NULL DEFAULT 'gpt-4o-mini',
    base_url TEXT NOT NULL DEFAULT 'https://api.openai.com/v1',
    api_key TEXT NOT NULL DEFAULT '',
    oauth_client_id TEXT NOT NULL DEFAULT 'agentdesk-desktop-client',
    oauth_auth_url TEXT NOT NULL DEFAULT 'https://auth.openai.com/oauth/authorize',
    oauth_token_url TEXT NOT NULL DEFAULT 'https://auth.openai.com/oauth/token',
    oauth_access_token TEXT NOT NULL DEFAULT '',
    oauth_refresh_token TEXT NOT NULL DEFAULT '',
    autonomy_level TEXT NOT NULL DEFAULT 'human_approved_exec',
    redact_pii BOOLEAN NOT NULL DEFAULT true,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS ai_conversations (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    device_id UUID NULL REFERENCES devices(id) ON DELETE SET NULL,
    ticket_id UUID NULL REFERENCES support_requests(id) ON DELETE SET NULL,
    prompt TEXT NOT NULL,
    reply TEXT NOT NULL,
    provider_used TEXT NOT NULL,
    proposed_actions JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS doc_pages (
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    slug TEXT NOT NULL,
    title TEXT NOT NULL,
    body_markdown TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, slug)
);

CREATE INDEX IF NOT EXISTS idx_device_telemetry_dev ON device_telemetry(device_id, collected_at DESC);
CREATE INDEX IF NOT EXISTS idx_rmm_commands_dev ON rmm_commands(device_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_ticket_comments_tkt ON ticket_comments(ticket_id, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_calendar_tech_time ON calendar_events(organization_id, technician_id, starts_at, ends_at);
CREATE INDEX IF NOT EXISTS idx_ai_conv_org ON ai_conversations(organization_id, created_at DESC);
