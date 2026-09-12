CREATE TABLE operation_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    operation_id uuid NOT NULL REFERENCES operations(id) ON DELETE CASCADE,
    message text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT operation_events_message_safe CHECK (length(message) > 0 AND length(message) <= 500)
);

CREATE INDEX operation_events_operation_idx ON operation_events (operation_id, id);
