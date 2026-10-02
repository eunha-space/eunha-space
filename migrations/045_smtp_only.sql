-- Keep legacy columns for rollback; new application code neither reads nor
-- renders them. Existing API credentials also authenticate to this provider's
-- standard SMTP service. Never overwrite an owner's SMTP configuration.
UPDATE tenant_configuration
SET smtp_host='smtp.resend.com',smtp_port=465,smtp_username='resend',
    smtp_password=resend_api_key,smtp_from=resend_from
WHERE smtp_host IS NULL AND resend_api_key<>'';

ALTER TABLE tenant_configuration ALTER COLUMN resend_api_key SET DEFAULT '';
ALTER TABLE tenant_configuration ALTER COLUMN resend_from SET DEFAULT '';
