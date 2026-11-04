-- The language an account reads the portal and its email in. It starts as
-- what the browser asked for and the customer can change it.
ALTER TABLE users ADD COLUMN locale text CHECK (locale IN ('en', 'ko'));
-- An email is sent in the language it was written in, under the service's
-- name in that language.
ALTER TABLE billing_emails ADD COLUMN locale text NOT NULL DEFAULT 'en' CHECK (locale IN ('en', 'ko'));
