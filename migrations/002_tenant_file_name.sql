ALTER TABLE tenants ADD COLUMN tenant_file_name text;

UPDATE tenants
SET tenant_file_name = replace(domain, '.', '-') || '.toml'
WHERE tenant_file_name IS NULL;

ALTER TABLE tenants ALTER COLUMN tenant_file_name SET NOT NULL;

ALTER TABLE tenants
ADD CONSTRAINT tenants_tenant_file_name_safe
CHECK (tenant_file_name ~ '^[A-Za-z0-9.-]+\.toml$');
