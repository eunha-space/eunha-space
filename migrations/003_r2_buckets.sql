CREATE TABLE r2_buckets (
    tenant_id uuid PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    bucket_name text NOT NULL UNIQUE,
    lifecycle_state text NOT NULL DEFAULT 'ready'
        CHECK (lifecycle_state IN ('provisioning', 'ready', 'deprovisioning', 'deleted', 'failed')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT r2_buckets_bucket_name_safe
        CHECK (bucket_name ~ '^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$')
);
