-- What a plan promises beyond its price, in the units customers are shown. Null
-- states no limit. Nothing measures or enforces these yet.
ALTER TABLE plans
    ADD COLUMN user_limit bigint CHECK (user_limit IS NULL OR user_limit >= 0),
    ADD COLUMN media_gb bigint CHECK (media_gb IS NULL OR media_gb >= 0),
    ADD COLUMN database_gb bigint CHECK (database_gb IS NULL OR database_gb >= 0),
    ADD COLUMN monthly_activities bigint
        CHECK (monthly_activities IS NULL OR monthly_activities >= 0);
