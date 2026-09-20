-- An imported tenant is an existing Mastodon instance that goes on being
-- itself: its domain, its accounts, its signing keys, its media. Creation
-- generates everything a tenant needs and an import is the opposite act, so
-- what has to be recorded is the provenance that makes the tenant
-- irreplaceable.
CREATE TABLE tenant_imports (
    tenant_id uuid PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    -- The custom-format dump on the runtime host, read by `eunha
    -- import-mastodon`. The control plane never opens it; it only constrains
    -- the shape of the path, which goes to a command as an argument.
    dump_path text NOT NULL,
    -- The instance's `public/system` tree, staged on the runtime host beside
    -- the dump. Mastodon's on-disk media layout is its object keys, and the
    -- database holds a file's name rather than its address, so the media has
    -- to arrive under the keys the instance already minted. Null when the
    -- instance keeps its media where it already is, which is the other way of
    -- satisfying the same requirement.
    media_path text,
    -- What the restore and the upload reported: the domain the data was proved
    -- to belong to, the row counts that say the instance arrived whole, and the
    -- file counts that say its media did. Operational evidence only — neither
    -- command prints a secret, and none is kept here.
    report jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT tenant_imports_dump_path_absolute CHECK (dump_path ~ '^/[A-Za-z0-9._/-]+$'),
    CONSTRAINT tenant_imports_media_path_absolute CHECK (media_path IS NULL OR media_path ~ '^/[A-Za-z0-9._/-]+$')
);

-- Restoring the dump sits between the empty migrated database and the render
-- that would otherwise put an empty instance at a domain whose accounts exist.
-- The media follows the render, because the rendered file is what says where
-- the instance's storage is, and precedes the reload, because an instance that
-- is serving should not be the one discovering its pictures are missing. The
-- two are separate steps so that a media library that fails partway is retried
-- on its own rather than by restoring the database again.
ALTER TABLE provisioning_steps DROP CONSTRAINT provisioning_steps_step_check;
ALTER TABLE provisioning_steps ADD CONSTRAINT provisioning_steps_step_check
    CHECK (step IN ('database', 'migrate', 'import', 'render', 'media', 'reload', 'health', 'routing', 'owner'));
