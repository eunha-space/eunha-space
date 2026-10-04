-- Operator identities remain for durable audit references; authentication lives
-- exclusively in customers and customer_sessions.
ALTER TABLE customers ADD COLUMN operator_id uuid UNIQUE REFERENCES operators(id);

-- Only email identities can participate in customer authentication. Retain
-- existing customer passwords and verification status when identities overlap.
INSERT INTO customers(id,email,password_hash,email_verified_at)
SELECT id,lower(username),password_hash,now() FROM operators
WHERE username LIKE '%@%' AND length(username) <= 254
ON CONFLICT(email) DO NOTHING;

UPDATE customers c SET operator_id=o.id FROM operators o
WHERE c.email=lower(o.username);

DROP TABLE sessions;
ALTER TABLE operators DROP COLUMN password_hash;
