ALTER TABLE runtimes
    ADD COLUMN router_enabled boolean NOT NULL DEFAULT false,
    ADD COLUMN blue_bind_address text,
    ADD COLUMN green_bind_address text;

WITH numbered AS (
    SELECT id, row_number() OVER (ORDER BY created_at,id) - 1 AS offset
    FROM runtimes
)
UPDATE runtimes r
SET blue_bind_address = regexp_replace(r.bind_address, ':[0-9]+$', '') || ':' || (61000 + numbered.offset * 2),
    green_bind_address = regexp_replace(r.bind_address, ':[0-9]+$', '') || ':' || (61001 + numbered.offset * 2)
FROM numbered
WHERE numbered.id = r.id;

ALTER TABLE runtimes ALTER COLUMN blue_bind_address SET NOT NULL;
ALTER TABLE runtimes ALTER COLUMN green_bind_address SET NOT NULL;
ALTER TABLE runtimes ADD CONSTRAINT runtimes_blue_bind_address_key UNIQUE (blue_bind_address);
ALTER TABLE runtimes ADD CONSTRAINT runtimes_green_bind_address_key UNIQUE (green_bind_address);
