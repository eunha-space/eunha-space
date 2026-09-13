CREATE TABLE customers (
    id uuid PRIMARY KEY,
    email text NOT NULL UNIQUE CHECK (email = lower(email)),
    password_hash text NOT NULL,
    email_verified_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE customer_sessions (
    token_hash bytea PRIMARY KEY,
    customer_id uuid NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX customer_sessions_expires_at_idx ON customer_sessions (expires_at);

CREATE TABLE customer_email_verifications (
    token_hash bytea PRIMARY KEY,
    customer_id uuid NOT NULL REFERENCES customers(id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX customer_email_verifications_customer_idx
    ON customer_email_verifications (customer_id);

ALTER TABLE tenants ADD COLUMN customer_id uuid REFERENCES customers(id);
CREATE INDEX tenants_customer_id_idx ON tenants (customer_id);

ALTER TABLE operations
    ADD COLUMN requested_by_customer uuid REFERENCES customers(id);

ALTER TABLE operations ADD CONSTRAINT operations_one_requester
    CHECK (NOT (requested_by IS NOT NULL AND requested_by_customer IS NOT NULL));
