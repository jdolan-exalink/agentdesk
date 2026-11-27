CREATE TABLE IF NOT EXISTS device_alerts (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    fingerprint TEXT NOT NULL,
    severity TEXT NOT NULL CHECK (severity IN ('warning', 'critical')),
    status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'acknowledged', 'resolved')),
    summary TEXT NOT NULL,
    current_value DOUBLE PRECISION NOT NULL,
    threshold DOUBLE PRECISION NOT NULL,
    unit TEXT NOT NULL DEFAULT '%',
    occurrence_count INTEGER NOT NULL DEFAULT 1 CHECK (occurrence_count > 0),
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    acknowledged_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    acknowledged_at TIMESTAMPTZ NULL,
    resolved_at TIMESTAMPTZ NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_device_alerts_active_fingerprint
    ON device_alerts (device_id, fingerprint)
    WHERE status IN ('open', 'acknowledged');

CREATE INDEX IF NOT EXISTS idx_device_alerts_org_status_time
    ON device_alerts (organization_id, status, last_seen_at DESC);

CREATE INDEX IF NOT EXISTS idx_device_alerts_device_time
    ON device_alerts (device_id, created_at DESC);
