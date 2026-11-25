-- Direct peer ID + random password connections for desktop dual-mode clients

ALTER TABLE devices
    ADD COLUMN IF NOT EXISTS access_password_hash TEXT NULL;

ALTER TABLE devices
    DROP CONSTRAINT IF EXISTS devices_kind_check;

ALTER TABLE devices
    ADD CONSTRAINT devices_kind_check CHECK (kind IN ('host', 'quicksupport', 'desktop'));

ALTER TABLE remote_sessions
    ALTER COLUMN technician_id DROP NOT NULL;

ALTER TABLE remote_sessions
    ADD COLUMN IF NOT EXISTS caller_label TEXT NOT NULL DEFAULT '';
