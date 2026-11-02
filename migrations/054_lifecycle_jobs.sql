-- Lifecycle work runs from this table rather than from Temporal. A job is
-- requested in the same transaction as the tenant state change that needs it,
-- and a tenant has at most one live job of each kind. `generation` counts the
-- requests a running job has yet to see: one that arrives mid-run makes the job
-- run again rather than finish.
CREATE TABLE lifecycle_jobs (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    kind text NOT NULL CHECK (kind IN ('provision', 'deprovision', 'purge', 'domain_change')),
    generation bigint NOT NULL DEFAULT 0,
    failures integer NOT NULL DEFAULT 0,
    run_after timestamptz NOT NULL DEFAULT now(),
    lease_until timestamptz,
    last_error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz
);
CREATE UNIQUE INDEX lifecycle_jobs_one_live ON lifecycle_jobs (tenant_id, kind)
    WHERE finished_at IS NULL;
CREATE INDEX lifecycle_jobs_due ON lifecycle_jobs (run_after)
    WHERE finished_at IS NULL;
CREATE INDEX lifecycle_jobs_latest ON lifecycle_jobs (tenant_id, created_at DESC);

-- When a runtime may next be reconciled. A pass pushes it past its own timeout
-- on the way in, so only one is in flight, and a minute ahead on the way out.
ALTER TABLE runtimes ADD COLUMN reconcile_next_at timestamptz NOT NULL DEFAULT now();

-- Adopt the work Temporal was doing. Every step is safe to run again, and one
-- that already finished skips itself, so a job for something Temporal completed
-- in the meantime ends without doing anything.
INSERT INTO lifecycle_jobs(id,tenant_id,kind)
SELECT gen_random_uuid(), id,
       CASE lifecycle_state WHEN 'provisioning' THEN 'provision'
                            WHEN 'deprovisioning' THEN 'deprovision'
                            ELSE 'purge' END
FROM tenants
WHERE lifecycle_state IN ('provisioning','deprovisioning','purging');
INSERT INTO lifecycle_jobs(id,tenant_id,kind)
SELECT gen_random_uuid(), tenant_id, 'domain_change'
FROM domain_changes
WHERE status='running';

-- The purge sweep acts on any retained tenant past its date, where a Temporal
-- timer only ever acted on its own tenant. One already past it here is one whose
-- timer did not purge it, for a reason nobody has looked at, so it gets a fresh
-- retention period instead of being destroyed by the first sweep.
UPDATE tenants SET purge_scheduled_for=now()+interval '7 days',updated_at=now()
WHERE lifecycle_state='retained' AND purge_scheduled_for<=now();

-- `temporal_dispatches` stays until this release has proved itself: the release
-- before it, which a failed deploy restores, still reads and writes it.
