-- Purging is the one operation that destroys a tenant. Only an operator can
-- start it, only for a retained tenant, and it runs as durable, retried steps:
-- remove its Cloudflare hostnames, drop its database and role, delete its media
-- and Redis keys, and discard its configuration. The tenant row stays, marked
-- purged, so its domain is never handed to anyone else — remote servers still
-- remember the accounts that lived there.
ALTER TABLE tenants DROP CONSTRAINT tenants_lifecycle_state_check;
ALTER TABLE tenants ADD CONSTRAINT tenants_lifecycle_state_check
    CHECK (lifecycle_state IN (
        'provisioning', 'ready', 'failed', 'deprovisioning', 'retained', 'purging', 'purged'
    ));

ALTER TABLE tenants ADD COLUMN purged_at timestamptz;

CREATE TABLE purge_steps (
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    step text NOT NULL CHECK (step IN ('cloudflare', 'database', 'media', 'redis', 'configuration')),
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'succeeded', 'failed')),
    attempts integer NOT NULL DEFAULT 0,
    error text,
    started_at timestamptz,
    finished_at timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, step)
);
