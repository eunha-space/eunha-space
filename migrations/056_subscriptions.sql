-- Customer subscriptions billed through TossPayments automatic payments (빌링).
--
-- A subscription is one per user. Its billing key is what charges the card and
-- is a secret: it never leaves the server, through an API, a log, or an
-- operation event. A period is `anchor_at + periods_paid months` to one month
-- later, so a renewal on the 31st does not drift to the 28th for good.
CREATE TABLE subscriptions (
    user_id uuid PRIMARY KEY REFERENCES users(id),
    -- The identifier TossPayments files the billing key under. Random rather
    -- than the user id, because a customer key should not be guessable.
    customer_key uuid NOT NULL UNIQUE,
    status text NOT NULL DEFAULT 'incomplete'
        CHECK (status IN ('incomplete', 'active', 'past_due', 'canceled', 'lapsed')),
    billing_key text,
    card_company text,
    card_number text,
    amount bigint NOT NULL CHECK (amount > 0),
    anchor_at timestamptz NOT NULL DEFAULT now(),
    periods_paid integer NOT NULL DEFAULT 0 CHECK (periods_paid >= 0),
    current_period_end timestamptz,
    cancel_at_period_end boolean NOT NULL DEFAULT false,
    -- Renewal attempts that a card declined in a row.
    failed_attempts integer NOT NULL DEFAULT 0,
    -- When a charge is next due; null when nothing is to be charged.
    next_attempt_at timestamptz,
    -- A charge in flight holds this; another control plane leaves it alone
    -- until it passes.
    lease_until timestamptz,
    -- The card company's or TossPayments' customer-facing reason, never ours.
    last_error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX subscriptions_due ON subscriptions (next_attempt_at)
    WHERE next_attempt_at IS NOT NULL;

-- Every charge, requested before TossPayments is asked, so one interrupted
-- mid-request is asked about again under the same order id and idempotency key
-- rather than charged twice.
CREATE TABLE subscription_payments (
    order_id text PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id),
    amount bigint NOT NULL CHECK (amount > 0),
    period integer NOT NULL CHECK (period >= 0),
    period_start timestamptz NOT NULL,
    period_end timestamptz NOT NULL,
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'paid', 'failed')),
    payment_key text,
    receipt_url text,
    failure_code text,
    failure_message text,
    requested_at timestamptz NOT NULL DEFAULT now(),
    settled_at timestamptz
);
CREATE UNIQUE INDEX subscription_payments_one_pending ON subscription_payments (user_id)
    WHERE status = 'pending';
CREATE UNIQUE INDEX subscription_payments_one_paid_period ON subscription_payments (user_id, period_start)
    WHERE status = 'paid';
CREATE INDEX subscription_payments_history ON subscription_payments (user_id, requested_at DESC);

-- An instance taken offline because its owner's subscription ended. It keeps
-- its data with no purge scheduled, since nobody confirmed a deletion, and is
-- served again when the subscription is.
ALTER TABLE tenants ADD COLUMN billing_suspended_at timestamptz;
