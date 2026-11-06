-- Subscriptions paid through Polar, the merchant of record for cards
-- TossPayments cannot bill: those issued outside Korea. TossPayments stays the
-- provider for Korean cards, in won.
--
-- Polar keeps the schedule itself: it charges each renewal, retries a failed
-- one and ends the subscription when the retries run out. A Polar subscription
-- here is a mirror of Polar's, kept in step from its webhooks and a periodic
-- look, and the renewer never charges it.

-- The Polar product a plan is sold as, and the price Polar charges for it, read
-- from Polar when the plan is linked. Like a plan's won price, it never changes:
-- a new price is a new product on a new plan.
ALTER TABLE plans
    ADD COLUMN polar_product_id text UNIQUE,
    ADD COLUMN polar_amount bigint CHECK (polar_amount IS NULL OR polar_amount > 0),
    ADD COLUMN polar_currency text CHECK (polar_currency IS NULL OR polar_currency ~ '^[A-Z]{3}$'),
    ADD CONSTRAINT plans_polar_complete CHECK (
        (polar_product_id IS NULL) = (polar_amount IS NULL)
        AND (polar_product_id IS NULL) = (polar_currency IS NULL)
    );

-- Which provider bills a subscription, and in what currency its amount is. A
-- subscription is one per user whatever its provider, so a customer has at
-- most one running at a time. For a Polar subscription, `anchor_at` is the
-- start of its current period and `periods_paid` is 1, so the period is read
-- the same way as a TossPayments one; `next_attempt_at` stays null.
ALTER TABLE subscriptions
    ADD COLUMN provider text NOT NULL DEFAULT 'toss' CHECK (provider IN ('toss', 'polar')),
    ADD COLUMN currency text NOT NULL DEFAULT 'KRW' CHECK (currency ~ '^[A-Z]{3}$'),
    ADD COLUMN polar_subscription_id text UNIQUE,
    ADD COLUMN polar_customer_id text,
    -- When Polar's record was last read, so the periodic look starts with the
    -- stalest.
    ADD COLUMN polar_synced_at timestamptz,
    -- Whether Polar, when last read, was still billing it. A subscription that
    -- ended here, as a refund ends it, is revoked at Polar until this is false.
    ADD COLUMN polar_live boolean NOT NULL DEFAULT false;
CREATE INDEX subscriptions_polar_sync ON subscriptions (polar_synced_at NULLS FIRST)
    WHERE provider = 'polar';

-- A Polar payment's order id is Polar's order id. Its amount is what the
-- customer paid, tax included, in the payment's currency.
ALTER TABLE subscription_payments
    ADD COLUMN provider text NOT NULL DEFAULT 'toss' CHECK (provider IN ('toss', 'polar')),
    ADD COLUMN currency text NOT NULL DEFAULT 'KRW' CHECK (currency ~ '^[A-Z]{3}$'),
    -- When a refund was last asked of Polar, written before it is asked. Until
    -- Polar's refunds for the order say it was never made, it is not asked
    -- again: a second request could refund the order twice.
    ADD COLUMN refund_sent_at timestamptz;

-- Every Polar webhook received, before anything is decided about it, like
-- TossPayments' deliveries. Kept for five years with them.
CREATE TABLE polar_webhook_deliveries (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    received_at timestamptz NOT NULL DEFAULT now(),
    -- The `webhook-id` header. Polar sends an event again under the same id.
    webhook_id text,
    event_type text,
    -- Whether its signature verified. One that did not is kept but not acted on.
    verified boolean NOT NULL,
    payload text,
    http_status integer NOT NULL
);
CREATE INDEX polar_webhook_deliveries_received ON polar_webhook_deliveries (received_at DESC);
CREATE INDEX polar_webhook_deliveries_id ON polar_webhook_deliveries (webhook_id);
