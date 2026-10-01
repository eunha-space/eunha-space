# Tenant resource usage

The operator console at `/admin/usage` reads the authenticated `/api/usage`
endpoint every ten seconds while visible. Collection is on demand and shared
between viewers, with an in-memory ten-second cache and at most eight concurrent
probes. Each tenant probe has a four-second timeout. Large fleets can therefore
have a refresh cadence longer than ten seconds; each row shows its sample age.
No historical samples are persisted. The dashboard retains a bounded ten-minute
window in browser memory while the page is mounted. Choose a tenant from the
trend selector or click its domain in the table to graph storage, connections,
and transaction rate. Missing readings and collection pauses longer than thirty
seconds break the graph lines; cached samples are not added twice. Leaving or
reloading the page clears graph history.

The collector uses each tenant's configured direct PostgreSQL connection. It
reports database size, backend connections (excluding its own connection), active
backends, and committed plus rolled-back transactions. The browser computes
transactions per second between samples, discarding counter resets. Transaction
counts include monitoring activity. Backend counts do not represent PgBouncer
client connections. No migrations or additional credentials are required.

Ready, failed, and retained tenants are sampled. Retained databases still consume
storage. Other lifecycle states are shown as not sampled. Failed probes return
null metrics rather than zero; totals cover successful probes only. Credentials
and probe error details are never returned or logged by the collector.

CPU and RAM belong to shared runtime processes today and cannot be attributed
accurately to tenants. Media storage, request bandwidth, CPU, and memory are not
measured here. Supporting those requires tenant-aware runtime counters or
isolation, and a metrics store for durable history. This dashboard does not
estimate those values from process-wide readings.
