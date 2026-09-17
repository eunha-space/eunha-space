-- Eunha's MULTITENANCY.md carries a measured profile for hosting many small
-- tenants: a zero-minimum pool whose connections close after 20 seconds, and
-- queue loops that sleep 300 seconds when idle. Closing them that often is not
-- only a slot budget — a connection that has served Eunha's query mix costs
-- 19-24 MiB privately, and that cost grows the longer it stays open.
--
-- 035 raised the poll ceiling above the pool timeout, which was necessary but
-- not enough: the control plane's own health sweep touched every tenant more
-- often than the timeout, so nothing was ever reaped. With the sweep now on a
-- cheap endpoint every 300 seconds, a 20-second timeout leaves an idle tenant
-- holding a fraction of one connection instead of its whole pool.
ALTER TABLE tenant_configuration
    ALTER COLUMN database_pool_idle_timeout_seconds SET DEFAULT 20,
    ALTER COLUMN queue_idle_poll_seconds SET DEFAULT 300;

UPDATE tenant_configuration SET database_pool_idle_timeout_seconds=20,updated_at=now()
    WHERE database_pool_idle_timeout_seconds=120;

UPDATE tenant_configuration SET queue_idle_poll_seconds=300,updated_at=now()
    WHERE queue_idle_poll_seconds=900;

UPDATE tenants SET config_revision=config_revision+1,updated_at=now()
    WHERE lifecycle_state NOT IN ('purging','purged');
