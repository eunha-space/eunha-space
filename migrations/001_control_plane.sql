CREATE TABLE runtimes (
    id uuid PRIMARY KEY,
    name text NOT NULL UNIQUE,
    host text NOT NULL,
    tenant_directory text NOT NULL,
    service_label text NOT NULL,
    binary_path text NOT NULL,
    bind_address text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE tenants (
    id uuid PRIMARY KEY,
    domain text NOT NULL UNIQUE,
    title text NOT NULL,
    runtime_id uuid NOT NULL REFERENCES runtimes(id),
    desired_state text NOT NULL CHECK (desired_state IN ('running', 'disabled', 'deprovisioning')),
    observed_state text NOT NULL CHECK (observed_state IN ('unknown', 'healthy', 'unhealthy', 'disabled')),
    config_toml text NOT NULL,
    config_revision bigint NOT NULL DEFAULT 1,
    rendered_revision bigint,
    rendered_checksum text,
    imported_from text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE operators (
    id uuid PRIMARY KEY,
    username text NOT NULL UNIQUE,
    password_hash text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sessions (
    token_hash bytea PRIMARY KEY,
    operator_id uuid NOT NULL REFERENCES operators(id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE operations (
    id uuid PRIMARY KEY,
    tenant_id uuid REFERENCES tenants(id),
    kind text NOT NULL,
    status text NOT NULL CHECK (status IN ('pending', 'running', 'succeeded', 'failed')),
    requested_by uuid REFERENCES operators(id),
    detail jsonb NOT NULL DEFAULT '{}',
    error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    finished_at timestamptz
);

CREATE INDEX operations_recent_idx ON operations (created_at DESC);
CREATE INDEX operations_tenant_idx ON operations (tenant_id, created_at DESC);

