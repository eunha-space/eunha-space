-- Incremental release rollouts.
--
-- A runtime is one eunha process serving many tenants with one binary, so a
-- release reaches tenants runtime by runtime. Runtimes are grouped into release
-- channels, and a channel's position is the order a release reaches it: every
-- runtime in one channel takes it, the channel soaks, and only then does the
-- next channel start. A tenant's channel is its runtime's, and moving it to a
-- runtime in another channel is how it changes channel.
CREATE TABLE release_channels (
    id uuid PRIMARY KEY,
    name text NOT NULL UNIQUE CHECK (name ~ '^[a-z0-9][a-z0-9-]{0,31}$'),
    position integer NOT NULL,
    -- How long a channel runs a release, healthy, before the next one starts.
    soak_minutes integer NOT NULL DEFAULT 60 CHECK (soak_minutes BETWEEN 0 AND 10080),
    created_at timestamptz NOT NULL DEFAULT now(),
    -- Deferrable so that two channels can trade places in one transaction.
    CONSTRAINT release_channels_position_key UNIQUE (position) DEFERRABLE INITIALLY IMMEDIATE
);

-- A runtime in no channel takes no automatic release; an operator deploys it
-- by hand. Deleting a channel that still has runtimes is refused.
ALTER TABLE runtimes ADD COLUMN channel_id uuid REFERENCES release_channels(id);

-- The fleet keeps behaving as it did: every existing runtime goes in `stable`,
-- and `canary` waits, empty, for a runtime to be put in it.
INSERT INTO release_channels(id,name,position,soak_minutes) VALUES
    (gen_random_uuid(),'canary',0,60),
    (gen_random_uuid(),'stable',1,0);
UPDATE runtimes SET channel_id=(SELECT id FROM release_channels WHERE name='stable');

-- One build making its way through the channels. Only one is in flight: a newer
-- build supersedes it, and starts again from the first channel.
CREATE TABLE release_rollouts (
    id uuid PRIMARY KEY,
    git_sha text NOT NULL UNIQUE CHECK (git_sha ~ '^[0-9a-f]{40}$'),
    status text NOT NULL DEFAULT 'rolling'
        CHECK (status IN ('rolling', 'paused', 'halted', 'completed', 'superseded')),
    -- Failed releases before this do not count against the rollout, so that
    -- resuming a halted one gives its runtimes their attempts back.
    attempts_since timestamptz NOT NULL DEFAULT now(),
    -- Why it halted, or what it is waiting for, in fixed text.
    reason text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz
);
CREATE UNIQUE INDEX release_rollouts_one_active ON release_rollouts ((true))
    WHERE status IN ('rolling', 'paused', 'halted');

-- The runtimes running a newer build than a rollout's, deployed by hand: the
-- rollout counts them as done rather than take them back to its own. A build
-- is newer when it was first built later; before a rollout's own build has
-- been built anywhere, when the rollout started.
CREATE FUNCTION runtimes_ahead(rollout uuid) RETURNS TABLE (runtime_id uuid)
LANGUAGE sql STABLE AS $$
    SELECT r.id FROM runtimes r
    JOIN release_rollouts o ON o.id = rollout
    JOIN eunha_releases d ON d.git_sha = r.deployed_ref
    WHERE r.deployed_ref IS DISTINCT FROM o.git_sha
      AND d.created_at > COALESCE(
          (SELECT created_at FROM eunha_releases WHERE git_sha = o.git_sha),
          o.created_at)
$$;

-- How far a rollout has got in each channel.
CREATE TABLE release_rollout_stages (
    rollout_id uuid NOT NULL REFERENCES release_rollouts(id) ON DELETE CASCADE,
    channel_id uuid NOT NULL REFERENCES release_channels(id) ON DELETE CASCADE,
    started_at timestamptz NOT NULL DEFAULT now(),
    -- Every runtime in the channel that follows rollouts runs the release.
    completed_at timestamptz,
    -- An operator let the rollout past this channel's soak and health hold.
    promoted_at timestamptz,
    promoted_by uuid REFERENCES users(id),
    PRIMARY KEY (rollout_id, channel_id)
);

-- What a release found when it checked every tenant on its runtime, which is
-- what holds a rollout back when the release broke some of them.
ALTER TABLE runtime_deployments
    ADD COLUMN rollout_id uuid REFERENCES release_rollouts(id),
    ADD COLUMN tenants_checked integer,
    ADD COLUMN tenants_unhealthy integer;
CREATE INDEX runtime_deployments_rollout_idx ON runtime_deployments (rollout_id, runtime_id)
    WHERE rollout_id IS NOT NULL;

-- Moving a served tenant to another runtime. The steps are migrate, start,
-- switch, route and stop: the tenant is started on its new runtime, becomes
-- its tenant, and is routed there before its old runtime lets it go, so it is
-- never unserved; for the minutes between, both processes serve it, which
-- eunha's job queue and scheduler leases allow.
CREATE TABLE tenant_moves (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    operation_id uuid NOT NULL REFERENCES operations(id),
    from_runtime_id uuid NOT NULL REFERENCES runtimes(id),
    to_runtime_id uuid NOT NULL REFERENCES runtimes(id),
    status text NOT NULL DEFAULT 'running'
        CHECK (status IN ('running', 'succeeded', 'failed', 'cancelled')),
    step text NOT NULL DEFAULT 'migrate'
        CHECK (step IN ('migrate', 'start', 'switch', 'route', 'stop', 'done')),
    -- An operator asked to stop it; honoured only before `switch` completes.
    cancel_requested boolean NOT NULL DEFAULT false,
    -- When every hostname of the tenant pointed at the new runtime's origin.
    routed_at timestamptz,
    error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    CHECK (from_runtime_id <> to_runtime_id)
);
-- A tenant moves once at a time. A move is open until `finished_at`: one that
-- failed before the tenant switched runtimes is undone and finished, but one
-- that failed after stays open, holding both runtimes, until an operator
-- retries it to the end.
CREATE UNIQUE INDEX tenant_moves_one_open ON tenant_moves (tenant_id)
    WHERE finished_at IS NULL;
CREATE INDEX tenant_moves_open_runtimes ON tenant_moves (from_runtime_id, to_runtime_id)
    WHERE finished_at IS NULL;
CREATE INDEX tenant_moves_latest ON tenant_moves (tenant_id, created_at DESC);

ALTER TABLE lifecycle_jobs DROP CONSTRAINT lifecycle_jobs_kind_check;
ALTER TABLE lifecycle_jobs ADD CONSTRAINT lifecycle_jobs_kind_check
    CHECK (kind IN ('provision', 'deprovision', 'purge', 'domain_change', 'runtime_move'));
