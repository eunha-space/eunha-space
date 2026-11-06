-- Whether new subscriptions are taken, for Korean cards through TossPayments
-- and for international cards through Polar. Turning one off stops new
-- sign-ups only: subscriptions that run keep renewing, changing plans and
-- changing cards, and administrators can still subscribe to test. One row.
CREATE TABLE billing_settings (
    id boolean PRIMARY KEY DEFAULT true CHECK (id),
    toss_signups boolean NOT NULL DEFAULT true,
    polar_signups boolean NOT NULL DEFAULT true,
    updated_at timestamptz NOT NULL DEFAULT now(),
    updated_by uuid REFERENCES users(id) ON DELETE SET NULL
);

INSERT INTO billing_settings DEFAULT VALUES;
