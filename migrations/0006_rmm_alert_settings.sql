CREATE TABLE IF NOT EXISTS rmm_alert_settings (
    organization_id UUID PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
    cpu_warning_pct DOUBLE PRECISION NOT NULL DEFAULT 85,
    cpu_critical_pct DOUBLE PRECISION NOT NULL DEFAULT 95,
    ram_warning_pct DOUBLE PRECISION NOT NULL DEFAULT 85,
    ram_critical_pct DOUBLE PRECISION NOT NULL DEFAULT 95,
    disk_free_warning_pct DOUBLE PRECISION NOT NULL DEFAULT 15,
    disk_free_critical_pct DOUBLE PRECISION NOT NULL DEFAULT 5,
    updated_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (cpu_warning_pct > 0 AND cpu_warning_pct < cpu_critical_pct AND cpu_critical_pct <= 100),
    CHECK (ram_warning_pct > 0 AND ram_warning_pct < ram_critical_pct AND ram_critical_pct <= 100),
    CHECK (disk_free_critical_pct >= 0 AND disk_free_critical_pct < disk_free_warning_pct AND disk_free_warning_pct <= 100)
);
