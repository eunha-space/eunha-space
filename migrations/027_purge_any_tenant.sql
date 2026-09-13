-- Operators can purge a tenant that was never deprovisioned — a live, failed
-- or stuck-provisioning one — so a purge first takes the tenant offline itself.
ALTER TABLE purge_steps DROP CONSTRAINT purge_steps_step_check;
ALTER TABLE purge_steps ADD CONSTRAINT purge_steps_step_check
    CHECK (step IN ('offline', 'cloudflare', 'database', 'media', 'redis', 'configuration'));
