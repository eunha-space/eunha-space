-- The merchant ID (MID) that issued a subscription's billing key. Only that
-- MID's secret key can charge it, so a key issued under another one, such as
-- the test MID before a switch to live keys, is not charged at all rather than
-- counted as the card's decline.
ALTER TABLE subscriptions ADD COLUMN toss_mid text;
