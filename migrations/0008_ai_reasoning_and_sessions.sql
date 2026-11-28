ALTER TABLE ai_settings
    ADD COLUMN IF NOT EXISTS reasoning_effort TEXT NOT NULL DEFAULT 'high';

ALTER TABLE ai_conversations
    ADD COLUMN IF NOT EXISTS chat_session_id UUID,
    ADD COLUMN IF NOT EXISTS model_used TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS reasoning_effort TEXT NOT NULL DEFAULT 'high';

CREATE INDEX IF NOT EXISTS idx_ai_conv_device_session
    ON ai_conversations (organization_id, device_id, chat_session_id, created_at ASC);
