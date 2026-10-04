# Site settings

Eunha reads site identity, registration policy, privacy policy, and terms from
its tenant database. Eunha.space retains their control-plane values as bootstrap
seeds and no longer installs them in runtime TOML.

| Previous instance TOML key | Tenant database destination |
| --- | --- |
| `title` | `settings.site_title` |
| `short_description` | `settings.site_short_description` (falls back to `description`) |
| `description` | `settings.site_extended_description` and `settings.site_description` |
| `contact_email` | `settings.site_contact_email` |
| `registrations_open`, `approval_required` | `settings.registrations_mode` (`none`, `open`, or `approved`) |
| `privacy_policy` | `settings.site_terms` |
| `terms_of_service` | A published version in `terms_of_services` |

Provisioning, restored imports, and configuration rendering run
`eunha settings import-config` using a temporary private configuration. The
command fills only missing settings and preserves existing published terms,
including values restored from Mastodon. After owner creation, it also seeds
`site_contact_username` if missing. Future site edits belong in Eunha's admin
interface; rendering does not overwrite them with control-plane seed values.

The temporary directory has mode 0700 and its file mode 0600; subprocess output
is suppressed and the directory is removed on success or failure. Runtime TOML
still carries domain names, aliases, previous domains, VAPID keys, and `icon_url`,
as well as infrastructure and SMTP settings.

Deploy an Eunha runtime supporting `settings import-config` and migration 023
before this control-plane update. Control-plane migration 051 queues running,
ready tenants for reconciliation to seed missing settings and remove obsolete
runtime keys. Import failure prevents replacement of the existing runtime file.
