-- Connection-quality telemetry sent by the desktop app (host and viewer roles).
CREATE TABLE IF NOT EXISTS client_telemetry (
    id BIGSERIAL PRIMARY KEY,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    line TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_client_telemetry_device_time ON client_telemetry (device_id, created_at DESC);
