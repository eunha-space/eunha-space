-- A stable operator-facing name is distinct from the address used to reach an
-- instance. Keep existing installations valid by initially using their unique
-- address as the display name.
ALTER TABLE instances ADD COLUMN name text;
UPDATE instances SET name = address;
ALTER TABLE instances ALTER COLUMN name SET NOT NULL;
ALTER TABLE instances ADD CONSTRAINT instances_name_key UNIQUE (name);
