-- Site settings are configured inside Eunha and live only in its database.
ALTER TABLE tenants
    DROP COLUMN title,
    DROP COLUMN description,
    DROP COLUMN short_description,
    DROP COLUMN contact_email,
    DROP COLUMN registrations_open,
    DROP COLUMN approval_required,
    DROP COLUMN privacy_policy,
    DROP COLUMN terms_of_service;
