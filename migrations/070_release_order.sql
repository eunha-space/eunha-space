-- Which of two eunha releases is newer is a question for eunha's commit graph,
-- not for when each was first built. Two commits' order never changes, so
-- each answer is kept here and asked for once.
CREATE TABLE eunha_release_order (
    base text NOT NULL,
    head text NOT NULL,
    status text NOT NULL CHECK (status IN ('identical', 'ahead', 'behind', 'diverged')),
    compared_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (base, head)
);

-- Ordered by build time; the control plane asks the graph instead.
DROP FUNCTION runtimes_ahead(uuid);
