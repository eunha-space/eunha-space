CREATE TABLE tenant_usage_samples (
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    sampled_at bigint NOT NULL,
    database_bytes bigint,
    connections bigint,
    active_connections bigint,
    transactions bigint,
    stats_reset text,
    PRIMARY KEY (tenant_id, sampled_at)
);
CREATE INDEX tenant_usage_samples_retention ON tenant_usage_samples(sampled_at);
