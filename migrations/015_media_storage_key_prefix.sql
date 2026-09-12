ALTER TABLE tenant_configuration
    ADD COLUMN media_storage_key_prefix text NOT NULL DEFAULT '',
    ADD CONSTRAINT tenant_configuration_media_storage_key_prefix_safe
        CHECK (
            media_storage_key_prefix = '' OR
            media_storage_key_prefix ~ '^[A-Za-z0-9_-]+(/[A-Za-z0-9_-]+)*$'
        );
