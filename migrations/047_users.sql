-- A single identity owns tenants, authenticates, authorizes admin access, and
-- attributes operations. Never rewrite already applied migrations.
ALTER TABLE customers RENAME TO users;
ALTER TABLE users ADD COLUMN is_admin boolean NOT NULL DEFAULT false;
ALTER TABLE users ADD COLUMN legacy_username text;
-- Old operator identities without a customer account are audit-only users.
ALTER TABLE users ALTER COLUMN email DROP NOT NULL;
UPDATE users u SET is_admin=true,legacy_username=o.username
FROM operators o WHERE u.operator_id=o.id;
INSERT INTO users(id,email,password_hash,email_verified_at,created_at,legacy_username)
SELECT o.id,NULL,NULL,NULL,o.created_at,o.username FROM operators o
WHERE NOT EXISTS(SELECT 1 FROM users u WHERE u.operator_id=o.id);
ALTER TABLE users ADD CONSTRAINT users_identity_check CHECK (
    email IS NOT NULL OR
    (legacy_username IS NOT NULL AND password_hash IS NULL AND
     email_verified_at IS NULL AND NOT is_admin)
);

-- Translate operator IDs into their linked user IDs before dropping the role
-- link. Unlinked historical IDs continue pointing to their audit-only users.
ALTER TABLE operations DROP CONSTRAINT operations_requested_by_fkey;
ALTER TABLE operations DROP CONSTRAINT operations_one_requester;
UPDATE operations op SET requested_by=u.id FROM users u
WHERE u.operator_id=op.requested_by;
UPDATE operations SET requested_by=COALESCE(requested_by,requested_by_customer);
ALTER TABLE operations DROP COLUMN requested_by_customer;
ALTER TABLE operations ADD CONSTRAINT operations_requested_by_fkey
    FOREIGN KEY(requested_by) REFERENCES users(id);
ALTER TABLE users DROP COLUMN operator_id;
DROP TABLE operators;

ALTER TABLE customer_sessions RENAME TO user_sessions;
ALTER TABLE user_sessions RENAME COLUMN customer_id TO user_id;
ALTER TABLE customer_email_verifications RENAME TO user_email_verifications;
ALTER TABLE user_email_verifications RENAME COLUMN customer_id TO user_id;
ALTER TABLE tenants RENAME COLUMN customer_id TO user_id;

-- Keep constraint and index names aligned with their tables and columns.
ALTER TABLE users RENAME CONSTRAINT customers_pkey TO users_pkey;
ALTER TABLE users RENAME CONSTRAINT customers_email_key TO users_email_key;
ALTER TABLE users RENAME CONSTRAINT customers_email_check TO users_email_check;
ALTER TABLE users RENAME CONSTRAINT customers_instance_limit_check TO users_instance_limit_check;
ALTER TABLE user_sessions RENAME CONSTRAINT customer_sessions_pkey TO user_sessions_pkey;
ALTER TABLE user_sessions RENAME CONSTRAINT customer_sessions_customer_id_fkey TO user_sessions_user_id_fkey;
ALTER TABLE user_email_verifications RENAME CONSTRAINT customer_email_verifications_pkey TO user_email_verifications_pkey;
ALTER TABLE user_email_verifications RENAME CONSTRAINT customer_email_verifications_customer_id_fkey TO user_email_verifications_user_id_fkey;
ALTER TABLE tenants RENAME CONSTRAINT tenants_customer_id_fkey TO tenants_user_id_fkey;
ALTER INDEX customer_sessions_expires_at_idx RENAME TO user_sessions_expires_at_idx;
ALTER INDEX customer_email_verifications_customer_idx RENAME TO user_email_verifications_user_idx;
ALTER INDEX tenants_customer_id_idx RENAME TO tenants_user_id_idx;
