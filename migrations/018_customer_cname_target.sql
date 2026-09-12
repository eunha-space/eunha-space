ALTER TABLE tenant_domains
    ALTER COLUMN cname_target SET DEFAULT 'customers.eunha.space';

UPDATE tenant_domains
SET cname_target = 'customers.eunha.space', updated_at = now()
WHERE cname_target = 't.eunha.space';

INSERT INTO reserved_subdomains(label,reason)
VALUES ('customers','Cloudflare for SaaS target')
ON CONFLICT(label) DO UPDATE SET reason = excluded.reason;
