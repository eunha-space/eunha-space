-- Eunha's benchmarks put a small tenant's useful pool at two connections, with
-- no measurable throughput gained between three and five, and a warm connection
-- costing 19-24 MiB privately. Five was chosen when a tenant appeared to be
-- using all ten of its connections; that traffic turned out to be the control
-- plane's own health sweep, which no longer takes a connection per statistic.
ALTER TABLE tenant_configuration
    ALTER COLUMN database_pool_max_connections SET DEFAULT 2;

UPDATE tenant_configuration SET database_pool_max_connections=2,updated_at=now()
    WHERE database_pool_max_connections=5;

-- The one instance serving enough public traffic to hold its pool open gets the
-- third connection the benchmarks allow for a demonstrably busy tenant. This is
-- a measurement, not a plan tier: promote another only on evidence.
UPDATE tenant_configuration c SET database_pool_max_connections=3,updated_at=now()
    FROM tenants t
    WHERE t.id=c.tenant_id AND t.domain='seoul.earth'
      AND c.database_pool_max_connections=2;

UPDATE tenants SET config_revision=config_revision+1,updated_at=now()
    WHERE lifecycle_state NOT IN ('purging','purged');
