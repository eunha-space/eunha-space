ALTER TABLE tenants
    ADD COLUMN lifecycle_state text NOT NULL DEFAULT 'ready'
        CHECK (lifecycle_state IN ('provisioning', 'ready', 'failed', 'deprovisioning', 'retained', 'purged'));

ALTER TABLE r2_buckets DROP CONSTRAINT r2_buckets_bucket_name_key;

CREATE TABLE provisioning_steps (
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    step text NOT NULL CHECK (step IN ('database', 'migrate', 'render', 'reload', 'health')),
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'succeeded', 'failed')),
    attempts integer NOT NULL DEFAULT 0,
    error text,
    started_at timestamptz,
    finished_at timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, step)
);

CREATE INDEX provisioning_steps_status_idx ON provisioning_steps (status, updated_at);
