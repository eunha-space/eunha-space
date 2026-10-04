-- Release primary names only after every purge step has completed. Retained
-- and disabled tenants still reserve their names so they can be restored.
ALTER TABLE tenants DROP CONSTRAINT tenants_domain_key;
CREATE UNIQUE INDEX tenants_domain_key ON tenants(domain)
    WHERE lifecycle_state <> 'purged';
