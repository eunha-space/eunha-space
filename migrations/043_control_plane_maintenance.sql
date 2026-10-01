-- A bounded lease prevents runtime rollouts while the control plane is replaced.
CREATE TABLE IF NOT EXISTS control_plane_maintenance (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    deploy_until timestamptz
);
INSERT INTO control_plane_maintenance(singleton) VALUES(true) ON CONFLICT DO NOTHING;
