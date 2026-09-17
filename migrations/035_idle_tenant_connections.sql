-- A tenant's queue loops back off when they find nothing, but only up to
-- `queue_idle_poll_seconds`, and every poll touches the database. With the poll
-- ceiling at 30s and the pool's idle timeout at 600s, no connection was ever
-- unused long enough to be reaped, so an idle tenant held its whole pool
-- forever and the server's cost was tenant count times pool size.
--
-- The runtime's own configuration documents the fix: raise the poll ceiling
-- above the pool's idle timeout. An idle tenant then holds a connection for the
-- timeout and nothing for the rest of the interval. Work does not wait on the
-- ceiling — enqueuing wakes a loop immediately, and the timed tasks sleep until
-- their next item is due — so what a longer ceiling delays is a delivery retry
-- coming due on an instance that is otherwise doing nothing.
ALTER TABLE tenant_configuration
    ALTER COLUMN queue_idle_poll_seconds SET DEFAULT 900,
    ALTER COLUMN database_pool_idle_timeout_seconds SET DEFAULT 120;

UPDATE tenant_configuration SET queue_idle_poll_seconds=900,updated_at=now()
    WHERE queue_idle_poll_seconds=30;

UPDATE tenant_configuration SET database_pool_idle_timeout_seconds=120,updated_at=now()
    WHERE database_pool_idle_timeout_seconds=600;

UPDATE tenants SET config_revision=config_revision+1,updated_at=now()
    WHERE lifecycle_state NOT IN ('purging','purged');
