-- Refunds. A request is recorded before TossPayments is asked, and the renewer
-- carries it through: looked up first, cancelled, and settled from what
-- TossPayments then reports. A refusal on the merits withdraws the request
-- (refund_requested_at goes back to null, refund_error says why), so the
-- customer can ask again and renewals, held back while it is open, resume.
ALTER TABLE subscription_payments
    ADD COLUMN refund_requested_at timestamptz,
    ADD COLUMN refund_requested_by uuid REFERENCES users(id),
    ADD COLUMN refund_next_attempt_at timestamptz,
    ADD COLUMN refunded_amount bigint NOT NULL DEFAULT 0 CHECK (refunded_amount >= 0),
    ADD COLUMN refunded_at timestamptz,
    ADD COLUMN refund_error text;
ALTER TABLE subscription_payments DROP CONSTRAINT subscription_payments_status_check;
ALTER TABLE subscription_payments ADD CONSTRAINT subscription_payments_status_check
    CHECK (status IN ('pending', 'paid', 'failed', 'expired', 'refunded'));
CREATE INDEX subscription_payments_refund_due ON subscription_payments (refund_next_attempt_at)
    WHERE refund_requested_at IS NOT NULL AND refunded_at IS NULL;

-- Billing keys nothing uses any more: a subscription that ended, a card that
-- was replaced. A key stays chargeable at TossPayments for as long as its card
-- is valid, so each is deleted there, retried with backoff; once TossPayments
-- confirms, the key itself is erased.
CREATE TABLE retired_billing_keys (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id uuid NOT NULL REFERENCES users(id),
    billing_key text,
    toss_mid text,
    retired_at timestamptz NOT NULL DEFAULT now(),
    attempts integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    last_error text,
    deleted_at timestamptz,
    CHECK ((deleted_at IS NULL) = (billing_key IS NOT NULL))
);
CREATE INDEX retired_billing_keys_due ON retired_billing_keys (next_attempt_at)
    WHERE deleted_at IS NULL;

-- Ended subscriptions still hold their keys.
INSERT INTO retired_billing_keys(user_id,billing_key,toss_mid)
SELECT user_id,billing_key,toss_mid FROM subscriptions
WHERE status IN ('canceled','lapsed') AND billing_key IS NOT NULL;
UPDATE subscriptions SET billing_key=NULL,toss_mid=NULL
WHERE status IN ('canceled','lapsed') AND billing_key IS NOT NULL;
