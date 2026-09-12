ALTER TABLE tenants ADD COLUMN aliases text[] NOT NULL DEFAULT '{}';

CREATE TABLE tenant_domains (
    id uuid PRIMARY KEY,
    tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    hostname text NOT NULL UNIQUE,
    kind text NOT NULL CHECK (kind IN ('provided', 'custom', 'legacy')),
    status text NOT NULL CHECK (status IN ('active', 'pending', 'failed', 'removing')),
    is_primary boolean NOT NULL DEFAULT false,
    cloudflare_id text UNIQUE,
    cloudflare_status text,
    ssl_status text,
    cname_target text NOT NULL DEFAULT 't.eunha.space',
    ownership_name text,
    ownership_value text,
    error text,
    attempts integer NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX tenant_domains_one_primary_idx ON tenant_domains (tenant_id) WHERE is_primary;
CREATE INDEX tenant_domains_pending_idx ON tenant_domains (status, updated_at);

CREATE TABLE reserved_subdomains (
    label text PRIMARY KEY CHECK (label ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    reason text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

INSERT INTO reserved_subdomains(label,reason) VALUES
    ('admin','infrastructure'),('api','infrastructure'),('assets','infrastructure'),
    ('auth','infrastructure'),('cdn','infrastructure'),('console','infrastructure'),
    ('customers','Cloudflare for SaaS target'),('docs','infrastructure'),
    ('mail','infrastructure'),('media','infrastructure'),('origin','infrastructure'),
    ('r2','object storage'),('status','infrastructure'),('support','infrastructure'),
    ('www','infrastructure');

INSERT INTO tenant_domains(id,tenant_id,hostname,kind,status,is_primary)
SELECT gen_random_uuid(),id,domain,'legacy','active',true FROM tenants;
