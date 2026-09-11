CREATE TABLE tenant_configuration (
    tenant_id uuid PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,

    database_url text NOT NULL,
    database_pool_max_connections integer NOT NULL DEFAULT 20,
    database_pool_min_connections integer NOT NULL DEFAULT 0,
    database_pool_acquire_timeout_seconds bigint NOT NULL DEFAULT 30,
    database_pool_idle_timeout_seconds bigint NOT NULL DEFAULT 600,

    redis_url text NOT NULL,
    redis_coordination_url text,
    redis_key_prefix text NOT NULL DEFAULT '',
    redis_process_metrics boolean NOT NULL DEFAULT true,

    smtp_host text,
    smtp_port integer,
    smtp_username text,
    smtp_password text,
    smtp_from text,
    resend_api_key text NOT NULL,
    resend_from text NOT NULL,

    active_record_encryption_primary_key text,
    active_record_encryption_key_derivation_salt text,
    software_update_url text,
    allowed_private_networks text[] NOT NULL DEFAULT '{}',
    sign_integrity_proofs boolean NOT NULL DEFAULT false,

    media_storage_region text NOT NULL,
    media_storage_endpoint text,
    media_storage_access_key_id text NOT NULL,
    media_storage_secret_access_key text NOT NULL,
    media_storage_base_url text NOT NULL,

    delivery_workers integer NOT NULL DEFAULT 1,
    delivery_batch bigint NOT NULL DEFAULT 50,
    delivery_concurrency integer NOT NULL DEFAULT 16,
    inbox_workers integer NOT NULL DEFAULT 1,
    inbox_batch bigint NOT NULL DEFAULT 20,
    inbox_concurrency integer NOT NULL DEFAULT 4,
    queue_idle_poll_seconds bigint NOT NULL DEFAULT 30,
    process_delivery_concurrency integer NOT NULL DEFAULT 256,

    max_concurrent_requests integer,
    process_max_tenants integer,
    process_database_connections bigint,

    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),

    CHECK ((smtp_host IS NULL AND smtp_port IS NULL AND smtp_username IS NULL AND smtp_password IS NULL AND smtp_from IS NULL)
        OR (smtp_host IS NOT NULL AND smtp_port IS NOT NULL AND smtp_username IS NOT NULL AND smtp_password IS NOT NULL AND smtp_from IS NOT NULL)),
    CHECK ((active_record_encryption_primary_key IS NULL AND active_record_encryption_key_derivation_salt IS NULL)
        OR (active_record_encryption_primary_key IS NOT NULL AND active_record_encryption_key_derivation_salt IS NOT NULL)),
    CHECK (database_pool_max_connections > 0),
    CHECK (database_pool_min_connections >= 0 AND database_pool_min_connections <= database_pool_max_connections),
    CHECK (database_pool_acquire_timeout_seconds > 0 AND database_pool_idle_timeout_seconds > 0)
);

ALTER TABLE tenants
    ADD COLUMN description text NOT NULL DEFAULT '',
    ADD COLUMN short_description text NOT NULL DEFAULT '',
    ADD COLUMN contact_email text,
    ADD COLUMN registrations_open boolean NOT NULL DEFAULT true,
    ADD COLUMN approval_required boolean NOT NULL DEFAULT false,
    ADD COLUMN vapid_private_key text,
    ADD COLUMN vapid_public_key text,
    ADD COLUMN icon_url text,
    ADD COLUMN privacy_policy text NOT NULL DEFAULT '',
    ADD COLUMN terms_of_service text NOT NULL DEFAULT '';
