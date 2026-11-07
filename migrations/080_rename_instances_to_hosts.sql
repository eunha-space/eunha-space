-- "Instance" is the customer's word for their Mastodon site, as it is in
-- Mastodon and in Eunha. A machine that runs runtimes is a host, the name
-- migration 012 gave it before 013 took the customer's word for it.
ALTER TABLE instances RENAME TO hosts;
ALTER INDEX instances_pkey RENAME TO hosts_pkey;
ALTER TABLE hosts RENAME CONSTRAINT instances_address_key TO hosts_address_key;
ALTER TABLE hosts RENAME CONSTRAINT instances_name_key TO hosts_name_key;

ALTER TABLE runtimes RENAME COLUMN instance_id TO host_id;
ALTER TABLE runtimes RENAME CONSTRAINT runtimes_instance_id_fkey TO runtimes_host_id_fkey;
ALTER INDEX runtimes_instance_id_idx RENAME TO runtimes_host_id_idx;

-- PostgreSQL 18 names NOT NULL constraints; a database first built on an
-- older release has none to rename.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conrelid = 'hosts'::regclass AND conname = 'instances_name_not_null') THEN
        ALTER TABLE hosts RENAME CONSTRAINT instances_name_not_null TO hosts_name_not_null;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conrelid = 'runtimes'::regclass AND conname = 'runtimes_instance_id_not_null') THEN
        ALTER TABLE runtimes RENAME CONSTRAINT runtimes_instance_id_not_null TO runtimes_host_id_not_null;
    END IF;
END $$;
