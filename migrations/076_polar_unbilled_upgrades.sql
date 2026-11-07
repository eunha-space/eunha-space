-- A Polar upgrade is billed with the next renewal (`prorate`), not at once.
-- Until that renewal is paid, the plan it moved from is kept here, so that
-- cancelling first takes the unpaid upgrade back instead of keeping it free.
ALTER TABLE subscriptions
    ADD COLUMN unbilled_from_plan_id uuid REFERENCES plans(id),
    ADD COLUMN unbilled_since timestamptz;
