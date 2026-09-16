-- Following an email link proves control of the address, which is what
-- authorizes replacing a forgotten password. A password sign-in proves only
-- that the password is known, so changing it there needs the current one.
ALTER TABLE customer_sessions
    ADD COLUMN from_email_link boolean NOT NULL DEFAULT false;
