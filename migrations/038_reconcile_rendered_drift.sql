-- A rendered file has always recorded the revision it came from, and nothing
-- has ever read it back: a tenant_configuration change reached a running
-- tenant only when an operator pressed render in /admin. Close that loop.

-- Migrations 034 through 037 each bumped config_revision by hand next to the
-- configuration they changed. A reconciler that decides from the revision
-- needs the database to guarantee it rather than every writer to remember it.
CREATE FUNCTION bump_tenant_config_revision() RETURNS trigger AS $$
BEGIN
    UPDATE tenants SET config_revision = config_revision + 1, updated_at = now()
    WHERE id = NEW.tenant_id;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tenant_configuration_revision
    AFTER UPDATE ON tenant_configuration
    FOR EACH ROW WHEN (OLD.* IS DISTINCT FROM NEW.*)
    EXECUTE FUNCTION bump_tenant_config_revision();

-- A tenant whose render fails for a reason that will not resolve itself — an
-- invalid stored domain, a runtime directory that is gone — must not be
-- retried on every pass forever. The count is cleared by a render that works,
-- and by an operator's explicit render, which is how a fixed tenant rejoins.
ALTER TABLE tenants
    ADD COLUMN reconcile_failures integer NOT NULL DEFAULT 0,
    ADD COLUMN reconcile_error text;

