# Tenant email delivery

Owners of ready instances with an active custom primary domain can configure
SMTP under **Email delivery** on the instance page. They supply a hostname,
port, sender email address, username, and password. Port 465 uses implicit TLS;
port 587 requires STARTTLS. Both verify the server certificate.

Credentials are write-only: every save supplies both credentials, and neither
is returned by the customer API or the operator configuration preview. Audit
records contain only the operation and actor, without SMTP values.

PostgreSQL stores the configuration. Saving it increments the tenant's
configuration revision in the same transaction and clears reconciliation
failures. Runtime reconciliation generates the private TOML file and reloads
Eunha. The save confirmation means the settings are stored, not that the
provider accepted a connection or delivered an email.

SMTP is the only mail transport for both Eunha instance mail and control-plane
verification mail. Missing SMTP settings fail delivery with a configuration
error. Configure control-plane SMTP through the `EUNHA_SPACE_SMTP_HOST`,
`EUNHA_SPACE_SMTP_PORT`, `EUNHA_SPACE_SMTP_USERNAME`, and
`EUNHA_SPACE_SMTP_PASSWORD` environment variables, plus `EUNHA_SPACE_EMAIL_FROM`.

Migration 045 maps legacy provider credentials to their SMTP endpoint for
existing tenants that have no SMTP settings. Existing custom SMTP settings stay
intact. Migration 049 removes the unused legacy provider columns; SMTP settings
remain intact. Deploy the updated control plane
before the SMTP-only runtime so reconciliation can generate SMTP settings.
