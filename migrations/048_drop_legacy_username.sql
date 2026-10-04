-- Remove the unused legacy name and its dependency in the identity check.
ALTER TABLE users DROP CONSTRAINT users_identity_check;
ALTER TABLE users DROP COLUMN legacy_username;
ALTER TABLE users ADD CONSTRAINT users_identity_check CHECK (
    email IS NOT NULL OR
    (password_hash IS NULL AND email_verified_at IS NULL AND NOT is_admin)
);
