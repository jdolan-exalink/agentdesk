ALTER TABLE device_groups
    ADD COLUMN IF NOT EXISTS owner_user_id UUID NULL
        REFERENCES users(id) ON DELETE SET NULL;

UPDATE device_groups AS g
SET owner_user_id = (
    SELECT u.id
    FROM users AS u
    WHERE u.organization_id = g.organization_id
    ORDER BY
        CASE WHEN u.role IN ('instance_admin', 'org_admin', 'admin') THEN 0 ELSE 1 END,
        u.created_at ASC,
        u.id ASC
    LIMIT 1
)
WHERE g.owner_user_id IS NULL
  AND EXISTS (
      SELECT 1 FROM users AS u WHERE u.organization_id = g.organization_id
  );

CREATE INDEX IF NOT EXISTS idx_device_groups_owner_user
    ON device_groups(owner_user_id);
