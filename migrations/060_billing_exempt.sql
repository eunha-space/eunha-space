-- An account an operator exempted from billing: like an administrator, it can
-- run instances without a subscription, and an ending subscription does not
-- take its instances offline.
ALTER TABLE users ADD COLUMN billing_exempt boolean NOT NULL DEFAULT false;
