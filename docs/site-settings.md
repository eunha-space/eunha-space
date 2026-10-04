# Site settings

Site settings belong only to each Eunha tenant database. Eunha.space does not
collect them during creation or import, store a bootstrap payload, or write
them to runtime TOML. Users configure them in Eunha's admin interface after
setup. Imported instances retain the settings restored from their dump.

| Previous instance TOML key | Tenant database destination |
| --- | --- |
| `title` | `settings.site_title` |
| `short_description` | `settings.site_short_description` |
| `description` | `settings.site_extended_description` and `settings.site_description` |
| `contact_email` | `settings.site_contact_email` |
| `registrations_open`, `approval_required` | `settings.registrations_mode` |
| `privacy_policy` | `settings.site_terms` |
| `terms_of_service` | Published versions in `terms_of_services` |

Migration 051 queued existing active tenants for the previous release's import
and reconciliation. Migration 052 removes all eight legacy columns from
`tenants`; `tenant_configuration` already contains only hosting settings.
Inventory identifies instances by their hostname.

Runtime TOML retains domain names, aliases, previous domains, VAPID keys,
`icon_url`, infrastructure, and SMTP settings. The owner's login email comes
from the customer's verified account and is separate from the site's public
contact email.
