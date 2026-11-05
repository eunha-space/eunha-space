-- What TossPayments sent and what its records say, kept apart from the payments
-- themselves. Kept for five years: as long as Korean e-commerce law keeps
-- payment records, and as long as TossPayments will look a payment up.

-- Every webhook received, before anything is decided about it. Billing keys are
-- secrets and are removed from the payload.
CREATE TABLE toss_webhook_deliveries (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    received_at timestamptz NOT NULL DEFAULT now(),
    event_type text,
    order_id text,
    -- Whether it carried a signature header. TossPayments' documentation
    -- disagrees with itself about which events have one.
    signed boolean NOT NULL,
    headers jsonb NOT NULL,
    payload text,
    http_status integer NOT NULL
);
CREATE INDEX toss_webhook_deliveries_received ON toss_webhook_deliveries (received_at DESC);

-- Each day in Korea compared with TossPayments' transactions for that day.
CREATE TABLE toss_transaction_checks (
    day date PRIMARY KEY,
    attempted_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    transactions integer,
    error text
);

-- What the comparison found that no code can settle by itself: an operator
-- decides what to do about money that moved without a matching record.
CREATE TABLE toss_mismatches (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    order_id text NOT NULL,
    kind text NOT NULL CHECK (kind IN (
        'approved_not_recorded', 'unknown_order', 'amount_differs', 'not_at_toss', 'still_pending'
    )),
    detail text NOT NULL,
    found_at timestamptz NOT NULL DEFAULT now(),
    resolved_at timestamptz,
    UNIQUE (order_id, kind)
);
CREATE INDEX toss_mismatches_open ON toss_mismatches (found_at DESC) WHERE resolved_at IS NULL;
