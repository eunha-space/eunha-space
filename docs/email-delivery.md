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

Eunha uses SMTP when configured and Resend otherwise. SMTP failures do not
fall back to Resend. Deploy the Eunha runtime with SMTP support alongside the
control-plane change; older runtimes accept the fields but ignore them.
