CREATE TABLE tenant_service_samples (
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    sampled_at bigint NOT NULL,
    epoch text,
    requests double precision,
    errors double precision,
    duration_sum double precision,
    duration_count double precision,
    aborted double precision,
    rejected double precision,
    in_flight double precision,
    pool double precision,
    pool_idle double precision,
    PRIMARY KEY (tenant_id, sampled_at)
);
CREATE INDEX tenant_service_samples_retention ON tenant_service_samples(sampled_at);
