-- A runtime release runs inside the control-plane process that queued it. If
-- that process exits — a deploy restarts it — the release stops, but its
-- operation stayed `running` and held the one-release-at-a-time lock forever.
-- A running release now records a heartbeat, and one whose heartbeat stops is
-- failed so the fleet can release again.
ALTER TABLE operations ADD COLUMN heartbeat_at timestamptz;
