-- Only one-way identifiers are retained; no addresses, emails, or credentials.
CREATE TABLE auth_rate_limits (
    key_hash bytea PRIMARY KEY,
    window_started_at timestamptz NOT NULL,
    attempts integer NOT NULL CHECK (attempts > 0)
);
CREATE INDEX auth_rate_limits_expiry ON auth_rate_limits(window_started_at);
