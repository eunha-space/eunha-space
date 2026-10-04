-- Re-render existing active tenants after importing their legacy site values
-- into Eunha's settings and terms tables. Saved values are never overwritten.
UPDATE tenants SET config_revision=config_revision+1,reconcile_failures=0,
    reconcile_error=NULL,updated_at=now()
WHERE lifecycle_state='ready' AND desired_state='running';
