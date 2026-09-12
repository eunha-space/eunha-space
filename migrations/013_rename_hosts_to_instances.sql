-- The console calls a machine an instance; the schema should say the same word.
ALTER TABLE hosts RENAME TO instances;
ALTER INDEX hosts_pkey RENAME TO instances_pkey;
ALTER TABLE instances RENAME CONSTRAINT hosts_address_key TO instances_address_key;

ALTER TABLE runtimes RENAME COLUMN host_id TO instance_id;
ALTER TABLE runtimes RENAME CONSTRAINT runtimes_host_id_fkey TO runtimes_instance_id_fkey;
ALTER TABLE runtimes RENAME CONSTRAINT runtimes_host_id_not_null TO runtimes_instance_id_not_null;
ALTER INDEX runtimes_host_id_idx RENAME TO runtimes_instance_id_idx;
