-- A custom domain can only be chosen when an instance is created, so setup now
-- waits for its DNS instead of giving up within a minute: a domain whose DNS is
-- not ready is re-checked with backoff for up to a day before it is dropped and
-- the instance continues on its provided hostname. The customer can also choose
-- to continue without it.
ALTER TABLE tenant_domains ADD COLUMN next_check_at timestamptz;

-- The creation-time custom domain that was dropped, so the instance page can say
-- why the instance lives at its provided hostname instead.
ALTER TABLE tenants ADD COLUMN abandoned_custom_domain text;
