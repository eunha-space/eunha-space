-- A customer instance's first account is its Owner, created the way Mastodon
-- creates a new server's: `eunha accounts create --role Owner`, run as the
-- final provisioning step under the username the customer chose. The password
-- is never stored; the customer area issues a fresh one on request.
ALTER TABLE tenants
    ADD COLUMN owner_username text CHECK (owner_username ~ '^[A-Za-z0-9_]{1,30}$'),
    ADD COLUMN owner_password_issued_at timestamptz;

ALTER TABLE provisioning_steps DROP CONSTRAINT provisioning_steps_step_check;
ALTER TABLE provisioning_steps ADD CONSTRAINT provisioning_steps_step_check
    CHECK (step IN ('database', 'migrate', 'render', 'reload', 'health', 'routing', 'owner'));
