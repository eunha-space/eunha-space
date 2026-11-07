-- A purged instance's handle domain is released with its primary name (see
-- 050_reuse_purged_domains.sql), so the same handles can be given to a new
-- instance. Retained and disabled instances still hold theirs.
ALTER TABLE tenants DROP CONSTRAINT tenants_handle_domain_key;
CREATE UNIQUE INDEX tenants_handle_domain_key ON tenants(handle_domain)
    WHERE lifecycle_state <> 'purged';
