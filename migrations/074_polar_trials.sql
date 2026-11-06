-- Free trials on Polar products. Polar keeps a trial itself: it takes the card
-- at checkout, charges nothing until the trial ends, and then bills the first
-- period. These columns only mirror what it holds, so that the trial can be
-- shown.

-- The trial a plan's Polar product gives a new subscriber, read from Polar
-- when the plan is linked or its product is read again. Null for none.
ALTER TABLE plans
    ADD COLUMN polar_trial_interval text
        CHECK (polar_trial_interval IN ('day', 'week', 'month', 'year')),
    ADD COLUMN polar_trial_interval_count integer CHECK (polar_trial_interval_count > 0),
    ADD CONSTRAINT plans_polar_trial_complete CHECK (
        (polar_trial_interval IS NULL) = (polar_trial_interval_count IS NULL)
    );

-- The trial a Polar subscription is in, set only while Polar says `trialing`.
-- Its start is kept because a plan change during a trial recomputes the end
-- from it, with the new product's trial length.
ALTER TABLE subscriptions
    ADD COLUMN trial_started_at timestamptz,
    ADD COLUMN trial_ends_at timestamptz,
    ADD CONSTRAINT subscriptions_trial_complete CHECK (
        trial_ends_at IS NULL OR (provider = 'polar' AND trial_started_at IS NOT NULL)
    );
