-- Runtime paths are a deterministic projection of tenant domains, not state.
ALTER TABLE tenants DROP COLUMN tenant_file_name;

-- Expired login sessions are pruned by the health reconciler.
CREATE INDEX sessions_expires_at_idx ON sessions (expires_at);
