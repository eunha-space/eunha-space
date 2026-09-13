-- A customer's deletion now takes the instance offline at once and keeps its
-- data for seven days, after which it is purged unless the customer restores it
-- or purges it sooner. The schedule therefore marks a purge, not a pending
-- deprovision; a deletion still waiting under the old meaning is dropped.
UPDATE tenants SET deletion_scheduled_for = NULL WHERE lifecycle_state <> 'retained';
ALTER TABLE tenants RENAME COLUMN deletion_scheduled_for TO purge_scheduled_for;
ALTER INDEX tenants_deletion_scheduled_idx RENAME TO tenants_purge_scheduled_idx;
