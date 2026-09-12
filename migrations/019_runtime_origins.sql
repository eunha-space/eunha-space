-- Route every tenant hostname to the runtime that owns it. Infrastructure
-- points each explicit origin hostname at that runtime's dedicated Tunnel.
ALTER TABLE runtimes ADD COLUMN origin_hostname text;

UPDATE runtimes r
SET origin_hostname = concat(r.name, '.', i.name, '.origin.eunha.space')
FROM instances i
WHERE i.id = r.instance_id;

ALTER TABLE runtimes ALTER COLUMN origin_hostname SET NOT NULL;
ALTER TABLE runtimes ADD CONSTRAINT runtimes_origin_hostname_key UNIQUE (origin_hostname);

ALTER TABLE tenant_domains ADD COLUMN routed_origin_hostname text;

-- Existing provided names are adopted into Cloudflare for SaaS on rollout.
UPDATE tenant_domains
SET status = 'pending', updated_at = now()
WHERE kind = 'provided';

ALTER TABLE provisioning_steps DROP CONSTRAINT provisioning_steps_step_check;
ALTER TABLE provisioning_steps ADD CONSTRAINT provisioning_steps_step_check
    CHECK (step IN ('database', 'migrate', 'render', 'reload', 'health', 'routing'));

INSERT INTO reserved_subdomains(label,reason)
VALUES ('origin','Runtime origin routing namespace')
ON CONFLICT (label) DO UPDATE SET reason=excluded.reason;
