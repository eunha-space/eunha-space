ALTER TABLE operations
    ADD COLUMN runtime_id uuid REFERENCES runtimes(id);

ALTER TABLE runtimes
    ADD COLUMN auto_deploy_enabled boolean NOT NULL DEFAULT false,
    ADD COLUMN deployed_ref text,
    ADD COLUMN release_checked_at timestamptz;

CREATE INDEX operations_runtime_idx ON operations (runtime_id, created_at DESC);

-- Release the fleet incrementally: there can be only one staging/switch in
-- flight, even when two operators or the automatic reconciler act together.
CREATE UNIQUE INDEX operations_one_active_fleet_release
    ON operations ((kind))
    WHERE kind = 'runtime_release' AND status IN ('pending', 'running');
