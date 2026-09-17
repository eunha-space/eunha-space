-- A runtime process holds one pool per tenant, and a tenant's background
-- delivery and inbox workers poll often enough that the pool's idle timeout
-- never reaps them. A tenant's cost is therefore its pool size, whether or not
-- anyone is reading it, and the server pays that for every tenant at once —
-- twice over while a blue/green switch drains both slots. The per-tenant pool
-- shrinks, and a process ceiling bounds what one runtime may hold no matter
-- how many tenants it serves.
ALTER TABLE tenant_configuration
    ALTER COLUMN database_pool_max_connections SET DEFAULT 5,
    ALTER COLUMN process_database_connections SET DEFAULT 80,
    ADD CONSTRAINT tenant_configuration_process_database_connections_check
        CHECK (process_database_connections IS NULL OR process_database_connections > 0);

-- Only tenants still carrying the previous default are moved, so a tenant
-- deliberately given a larger pool keeps it.
UPDATE tenant_configuration SET database_pool_max_connections=5,updated_at=now()
    WHERE database_pool_max_connections=10;

UPDATE tenant_configuration SET process_database_connections=80,updated_at=now()
    WHERE process_database_connections IS NULL;

-- A rendered tenant file still holds the old pool, so mark every tenant's
-- configuration ahead of what was rendered; the console shows the drift and an
-- operator render writes the new values out.
UPDATE tenants SET config_revision=config_revision+1,updated_at=now()
    WHERE lifecycle_state NOT IN ('purging','purged');
