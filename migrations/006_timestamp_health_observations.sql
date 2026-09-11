ALTER TABLE tenants
    ADD COLUMN observed_at timestamptz,
    ADD COLUMN observation_error text;
