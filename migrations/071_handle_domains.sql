-- An instance whose handles are on a domain apart from the one it is served
-- on: eunha's `domain` (Mastodon's LOCAL_DOMAIN) is the handle domain and
-- `web_domain` is `tenants.domain`. Only with a custom web domain, so the
-- actor ids leave with the customer. The handle domain is the customer's own
-- server, which redirects WebFinger to the web domain; it is not routed here.
ALTER TABLE tenants
    ADD COLUMN handle_domain text UNIQUE CHECK (handle_domain IS NULL OR handle_domain <> domain),
    ADD COLUMN handle_domain_verified_at timestamptz;
