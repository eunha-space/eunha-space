-- Workflows are requested in the same transaction as the tenant state change.
-- A worker may crash between commit and Temporal delivery; the dispatcher will
-- retry this row until the deterministic workflow ID has been accepted.
CREATE TABLE temporal_dispatches (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    workflow_kind text NOT NULL CHECK (workflow_kind IN ('provision', 'deprovision', 'purge')),
    workflow_id text NOT NULL UNIQUE,
    delivered_at timestamptz,
    attempts integer NOT NULL DEFAULT 0,
    last_error text,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX temporal_dispatches_pending_idx ON temporal_dispatches (created_at)
    WHERE delivered_at IS NULL;

-- Adopt in-flight work from the pre-outbox reconciler on first deployment.
INSERT INTO temporal_dispatches(id,tenant_id,workflow_kind,workflow_id)
SELECT gen_random_uuid(), id,
       CASE lifecycle_state WHEN 'provisioning' THEN 'provision'
                            WHEN 'purging' THEN 'purge'
                            ELSE 'deprovision' END,
       'tenant-' || CASE lifecycle_state WHEN 'provisioning' THEN 'provision'
                                          WHEN 'purging' THEN 'purge'
                                          ELSE 'deprovision' END || '-v3-' || id::text || '-' || gen_random_uuid()::text
FROM tenants
WHERE lifecycle_state IN ('provisioning','deprovisioning','retained','purging');
