-- Accounts exist briefly without a password while their email link is pending.
-- Verification establishes a session in which the customer sets a password.
ALTER TABLE customers ALTER COLUMN password_hash DROP NOT NULL;
