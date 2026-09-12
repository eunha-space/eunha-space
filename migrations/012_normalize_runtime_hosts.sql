-- A machine is an entity in its own right: several runtimes can share one, and
-- the console lists them. Lift the repeated host text out of runtimes.
CREATE TABLE hosts (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    address text NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now()
);

INSERT INTO hosts (address) SELECT DISTINCT host FROM runtimes;

ALTER TABLE runtimes ADD COLUMN host_id uuid REFERENCES hosts(id);
UPDATE runtimes SET host_id = hosts.id FROM hosts WHERE hosts.address = runtimes.host;
ALTER TABLE runtimes ALTER COLUMN host_id SET NOT NULL;
ALTER TABLE runtimes DROP COLUMN host;

CREATE INDEX runtimes_host_id_idx ON runtimes (host_id);
