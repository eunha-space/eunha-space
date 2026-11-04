-- Billing emails go out through an outbox: a row written in the same
-- transaction as what it is about, sent by the renewer with backoff. The
-- dedupe key keeps a receipt or a renewal reminder from being written twice.
CREATE TABLE billing_emails (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users(id),
    kind text NOT NULL CHECK (kind IN ('receipt', 'renewal_notice', 'refund')),
    dedupe_key text NOT NULL UNIQUE,
    subject text NOT NULL,
    body text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    attempts integer NOT NULL DEFAULT 0,
    last_error text,
    sent_at timestamptz,
    -- Not sent within three days: whatever it was about is long past.
    abandoned_at timestamptz
);
CREATE INDEX billing_emails_due ON billing_emails (next_attempt_at)
    WHERE sent_at IS NULL AND abandoned_at IS NULL;
