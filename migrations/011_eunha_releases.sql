CREATE TABLE eunha_releases (
    id uuid PRIMARY KEY,
    git_sha text NOT NULL UNIQUE CHECK (git_sha ~ '^[0-9a-f]{40}$'),
    source_ref text NOT NULL DEFAULT 'origin/main',
    artifact_path text NOT NULL,
    build_status text NOT NULL CHECK (build_status IN ('building', 'ready', 'failed')),
    artifact_checksum text,
    created_at timestamptz NOT NULL DEFAULT now(),
    built_at timestamptz,
    build_error text
);

CREATE TABLE runtime_deployments (
    id uuid PRIMARY KEY,
    runtime_id uuid NOT NULL REFERENCES runtimes(id),
    release_id uuid REFERENCES eunha_releases(id),
    operation_id uuid NOT NULL UNIQUE REFERENCES operations(id),
    previous_release_id uuid REFERENCES eunha_releases(id),
    trigger text NOT NULL CHECK (trigger IN ('manual', 'automatic')),
    status text NOT NULL CHECK (status IN ('queued', 'building', 'transferring', 'migrating', 'switching', 'healthy', 'rolled_back', 'failed')),
    error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    finished_at timestamptz
);

CREATE INDEX runtime_deployments_runtime_idx ON runtime_deployments (runtime_id, created_at DESC);
CREATE INDEX runtime_deployments_release_idx ON runtime_deployments (release_id, created_at DESC);
