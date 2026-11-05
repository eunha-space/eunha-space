-- An administrator signed in as another user. The session belongs to the user
-- it shows; the administrator it came from is kept so that it can be told
-- apart, ended when that administrator loses the role, and returned from.
ALTER TABLE user_sessions
    ADD COLUMN masquerading_admin_id uuid REFERENCES users(id) ON DELETE CASCADE;
