-- Changing a served instance's domain.
--
-- An instance with accounts used to keep its first domain forever: its
-- accounts' ActivityPub identities are that domain, and a verified custom
-- domain could only become an alias. A domain change now moves the instance:
-- its stored addresses are rewritten, it is served under the new domain, and
-- every account's followers are sent a Move to it. The old domain is served
-- no longer; `previous_domains` is what eunha names in each actor's
-- `alsoKnownAs`, which the followers' servers check before honouring a Move.
ALTER TABLE tenants ADD COLUMN previous_domains text[] NOT NULL DEFAULT '{}';

-- One row per change, the record an operator and the customer act on. The
-- workflow walks `step` through attach, rename, switch, move, deliver and
-- release; `batch_tag` is the eunha batch the Moves were sent in, and
-- `pending`/`given_up` how it last stood.
CREATE TABLE domain_changes (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    operation_id uuid NOT NULL REFERENCES operations(id),
    from_domain text NOT NULL,
    to_domain text NOT NULL,
    status text NOT NULL DEFAULT 'running'
        CHECK (status IN ('running', 'succeeded', 'failed', 'cancelled')),
    step text NOT NULL DEFAULT 'attach'
        CHECK (step IN ('attach', 'rename', 'switch', 'move', 'deliver', 'release', 'done')),
    batch_tag text,
    pending integer,
    given_up integer,
    error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz
);

-- An instance changes its domain once at a time.
CREATE UNIQUE INDEX domain_changes_one_running ON domain_changes (tenant_id)
    WHERE status = 'running';

ALTER TABLE temporal_dispatches DROP CONSTRAINT temporal_dispatches_workflow_kind_check;
ALTER TABLE temporal_dispatches ADD CONSTRAINT temporal_dispatches_workflow_kind_check
    CHECK (workflow_kind IN ('provision', 'deprovision', 'purge', 'domain_change'));
