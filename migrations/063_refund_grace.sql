-- When a refund took back the paid time a subscription was running on, the
-- moment its instances go offline unless the customer subscribes again.
ALTER TABLE subscriptions ADD COLUMN refund_grace_until timestamptz;
