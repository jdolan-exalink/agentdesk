-- AgentDesk v2 Migration 0004:
--   - Personal fixed password hash & owner user email binding on devices
--   - Device groups & granular sharing (read_only, read_write, full_control)
--   - 1-hour ephemeral direct reconnect grants (zero plaintext password storage on client)

ALTER TABLE devices
    ADD COLUMN IF NOT EXISTS personal_password_hash TEXT,
    ADD COLUMN IF NOT EXISTS owner_user_id UUID REFERENCES users(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_devices_owner_user ON devices(owner_user_id);

CREATE TABLE IF NOT EXISTS device_groups (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    owner_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (organization_id, owner_user_id, name)
);

CREATE TABLE IF NOT EXISTS device_group_shares (
    id UUID PRIMARY KEY,
    group_id UUID NOT NULL REFERENCES device_groups(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    permission TEXT NOT NULL CHECK (permission IN ('read_only', 'read_write', 'full_control')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (group_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_device_group_shares_user ON device_group_shares(user_id);

ALTER TABLE address_book_entries
    ADD COLUMN IF NOT EXISTS group_id UUID REFERENCES device_groups(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_address_book_group ON address_book_entries(group_id);

CREATE TABLE IF NOT EXISTS direct_reconnect_grants (
    id UUID PRIMARY KEY,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    caller_label TEXT NOT NULL DEFAULT 'Cliente AgentDesk',
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_reconnect_grants_device ON direct_reconnect_grants(device_id, expires_at);
