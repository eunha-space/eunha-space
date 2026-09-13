-- Customers delete an instance in two stages. Deleting schedules
-- deprovisioning a week out while the instance keeps running; the customer can
-- cancel, or bring it forward once it is scheduled. Deprovisioning takes the
-- instance offline and retains its database, media, configuration and hostnames,
-- so it can be restored until an operator purges it.
ALTER TABLE tenants ADD COLUMN deletion_scheduled_for timestamptz;

CREATE INDEX tenants_deletion_scheduled_idx
    ON tenants (deletion_scheduled_for)
    WHERE deletion_scheduled_for IS NOT NULL;

CREATE TABLE deprovisioning_steps (
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    step text NOT NULL CHECK (step IN ('unrender', 'reload', 'offline')),
    status text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'succeeded', 'failed')),
    attempts integer NOT NULL DEFAULT 0,
    error text,
    started_at timestamptz,
    finished_at timestamptz,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, step)
);
