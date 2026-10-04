ALTER TABLE tenants DROP COLUMN icon_url;

-- Remove the fallback from existing runtime files on the next reconciliation.
UPDATE tenants SET config_revision=config_revision+1,reconcile_failures=0,
    reconcile_error=NULL,updated_at=now()
WHERE lifecycle_state='ready' AND desired_state='running';
