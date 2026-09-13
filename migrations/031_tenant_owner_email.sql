ALTER TABLE tenants ADD COLUMN owner_email text;

UPDATE tenants t
SET owner_email = c.email
FROM customers c
WHERE c.id = t.customer_id AND t.owner_username IS NOT NULL;

ALTER TABLE tenants ADD CONSTRAINT tenants_owner_identity_complete
    CHECK ((owner_username IS NULL) = (owner_email IS NULL));
