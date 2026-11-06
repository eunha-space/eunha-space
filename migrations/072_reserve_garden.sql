-- `garden` is the example instance on the landing page and in the README, so
-- nobody may claim it.
INSERT INTO reserved_subdomains(label,reason)
VALUES ('garden','example on the landing page')
ON CONFLICT (label) DO UPDATE SET reason=excluded.reason;
