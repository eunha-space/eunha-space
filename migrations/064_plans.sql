-- What a customer can subscribe to. A plan's price never changes, because a
-- subscription and its upgrade charges are worked out from it: a new price is
-- a new plan, and the old one is retired by clearing `active`. A null
-- instance limit leaves the account's own limit in force.
CREATE TABLE plans (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name_en text NOT NULL CHECK (length(name_en) BETWEEN 1 AND 60),
    name_ko text NOT NULL CHECK (length(name_ko) BETWEEN 1 AND 60),
    amount bigint NOT NULL CHECK (amount > 0),
    instance_limit bigint CHECK (instance_limit IS NULL OR instance_limit >= 0),
    active boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now()
);

-- `plan_id` is what is being served. `pending_plan_id` is what the next
-- renewal moves to, or for a subscription not running, what checkout chose.
ALTER TABLE subscriptions
    ADD COLUMN plan_id uuid REFERENCES plans(id),
    ADD COLUMN pending_plan_id uuid REFERENCES plans(id);

-- An upgrade charge pays the difference for what is left of a period. It goes
-- through the same order, idempotency and lookup path as a renewal, and
-- remembers the plan it left so that refunding it can go back.
ALTER TABLE subscription_payments
    ADD COLUMN kind text NOT NULL DEFAULT 'renewal' CHECK (kind IN ('renewal', 'upgrade')),
    ADD COLUMN plan_id uuid REFERENCES plans(id),
    ADD COLUMN from_plan_id uuid REFERENCES plans(id),
    ADD COLUMN from_amount bigint;

-- Only the merchant that took a payment can give it back, and a subscription
-- forgets its merchant when it ends, so each payment remembers its own.
ALTER TABLE subscription_payments ADD COLUMN toss_mid text;
UPDATE subscription_payments p SET toss_mid=s.toss_mid FROM subscriptions s
WHERE s.user_id=p.user_id AND p.status IN ('pending', 'paid');
