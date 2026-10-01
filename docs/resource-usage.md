# Tenant resource usage

The operator console at `/admin/usage` reads the authenticated `/api/usage`
endpoint every ten seconds while visible. Collection runs independently in a
background loop, with at most eight concurrent probes and four seconds per
probe. API readers receive the latest cache immediately. Large fleets can have
a refresh cadence longer than ten seconds; each row shows its sample age.

Migration 042 stores numeric samples in PostgreSQL for 24 hours. The selected
tenant's authenticated history endpoint returns at most 120 samples from the
last ten minutes; the graphs restore that history after navigation or reload.
The latest persisted readings are restored into the cache after service restart.
Missing readings and collection pauses longer than thirty seconds break graph
lines. Cached samples are not added twice.

The page includes fleet-wide and hosting-server-wide metric cards and ten-minute
charts, alongside individual tenant trends. A hosting server groups all runtimes
with the same instance ID; it is the physical instance shown on the Instances
page. These totals describe its tenants' PostgreSQL usage, not total host CPU or
RAM. The authenticated `/api/usage/history` endpoint returns fleet history;
`?instance_id=<UUID>` scopes it to one hosting server. Aggregation uses current
tenant placement and eligibility, so moving a tenant changes its history's scope.

Aggregate history takes the latest sample per tenant in each ten-second bucket.
Transaction rates are calculated per tenant before summing, and counter resets
or gaps longer than thirty seconds invalidate the rate. Graphs omit incomplete
fleet/server totals. Cards sum only readings no older than thirty seconds, show
partial coverage explicitly, and display unavailable metrics as missing rather
than zero. The reporting count covers eligible, configured tenants.

The collector uses each tenant's configured direct PostgreSQL connection. It
reports database size, backend connections (excluding its own connection), active
backends, and committed plus rolled-back transactions. The browser computes
transactions per second between samples, discarding counter resets. Transaction
counts include monitoring activity. Backend counts do not represent PgBouncer
client connections. No additional tenant credentials are required.

Ready, failed, and retained tenants are sampled. Retained databases still consume
storage. Other lifecycle states are shown as not sampled. Failed probes return
null metrics rather than zero; totals cover successful probes only. Credentials
and probe error details are never returned or logged by the collector.

CPU and RAM belong to shared runtime processes today and cannot be attributed
accurately to tenants. Media storage, request bandwidth, CPU, and memory are not
measured here. Supporting those requires tenant-aware runtime counters or
isolation, and additional metrics storage. This dashboard does not
estimate those values from process-wide readings.
