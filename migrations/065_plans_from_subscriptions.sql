-- Prices used to come from the environment, which migrations cannot read, so a
-- running subscription's own amount says which plan it needs. New customers
-- can subscribe to these too until an operator stops offering them.
INSERT INTO plans(name_en,name_ko,amount)
SELECT DISTINCT 'Standard','기본',s.amount FROM subscriptions s
WHERE s.plan_id IS NULL AND s.status IN ('active','past_due')
  AND NOT EXISTS(SELECT 1 FROM plans p WHERE p.amount=s.amount);

UPDATE subscriptions s
SET plan_id=(SELECT p.id FROM plans p WHERE p.amount=s.amount ORDER BY p.created_at LIMIT 1)
WHERE s.plan_id IS NULL AND s.status IN ('active','past_due');
