-- Migration 045 converted existing credentials to SMTP. No runtime code reads
-- the legacy provider fields; retain SMTP values and remove the unused copies.
ALTER TABLE tenant_configuration
    DROP COLUMN resend_api_key,
    DROP COLUMN resend_from;
