-- Preserve storage ownership explicitly for future imports. Existing imports
-- use the tenant-owned prefix only when their media was copied into hosting.
ALTER TABLE tenant_imports ADD COLUMN external_media boolean NOT NULL DEFAULT false;
UPDATE tenant_imports i SET external_media=true FROM tenant_configuration c
WHERE c.tenant_id=i.tenant_id AND i.media_path IS NULL
  AND c.media_storage_key_prefix <> 't/' || i.tenant_id::text;

-- Slow capacity measurements are separate from the ten-second service samples.
CREATE TABLE tenant_usage (
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    sampled_at bigint NOT NULL,
    users bigint,
    local_media_bytes bigint,
    remote_media_bytes bigint,
    database_bytes bigint,
    PRIMARY KEY (tenant_id, sampled_at)
);
CREATE INDEX tenant_usage_retention ON tenant_usage(sampled_at);
-- UTC calendar days. Latest capacity reading of each day, not summed gauges.
CREATE TABLE tenant_usage_daily (LIKE tenant_usage INCLUDING ALL);
ALTER TABLE tenant_usage_daily ADD FOREIGN KEY (tenant_id) REFERENCES tenants(id) ON DELETE CASCADE;
ALTER TABLE tenant_usage_daily ADD COLUMN day date NOT NULL;
ALTER TABLE tenant_usage_daily ADD UNIQUE (tenant_id, day);
-- Observed counter deltas: gaps/reset intervals remain unknown, never zero.
ALTER TABLE tenant_service_samples ADD COLUMN deliveries double precision;
CREATE TABLE tenant_delivery_daily (
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    day date NOT NULL,
    deliveries double precision NOT NULL DEFAULT 0,
    observed_ms bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (tenant_id, day)
);
