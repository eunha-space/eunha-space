-- TossPayments keeps an idempotency key's first answer for 15 days, errors
-- included, so a charge sent again under its key can only replay a failure.
-- A pending payment whose charge went out is looked up by its order id
-- instead; one Toss never recorded is expired and a new order takes its place.
ALTER TABLE subscription_payments ADD COLUMN charge_attempted_at timestamptz;
-- Charges sent before this column existed were sent when requested.
UPDATE subscription_payments SET charge_attempted_at=requested_at WHERE status='pending';
ALTER TABLE subscription_payments DROP CONSTRAINT subscription_payments_status_check;
ALTER TABLE subscription_payments ADD CONSTRAINT subscription_payments_status_check
    CHECK (status IN ('pending', 'paid', 'failed', 'expired'));
