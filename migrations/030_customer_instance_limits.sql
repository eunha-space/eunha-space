ALTER TABLE customers
    ADD COLUMN instance_limit bigint DEFAULT 2
    CHECK (instance_limit IS NULL OR instance_limit >= 0);
