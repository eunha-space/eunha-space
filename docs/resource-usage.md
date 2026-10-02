# Tenant resource usage

The operator console at `/admin/usage` reads the authenticated `/api/usage`
endpoint every ten seconds while visible. Collection runs independently in a
background loop, with at most eight concurrent probes and four seconds per
probe. API readers receive the latest cache immediately. Large fleets can have
a refresh cadence longer than ten seconds; each row shows its sample age.

Migration 042 stores numeric samples in PostgreSQL for 28 days. The selected
tenant's authenticated history endpoint returns a bounded, range-dependent history; the graphs restore that history after navigation or reload.
The latest persisted readings are restored into the cache after service restart.
Missing readings and collection pauses longer than thirty seconds break graph
lines. Cached samples are not added twice.

The page includes fleet-wide and hosting-server-wide metric cards and range-selectable
charts, alongside individual tenant trends. A hosting server groups all runtimes
with the same instance ID; it is the physical instance shown on the Instances
page. These totals describe its tenants' PostgreSQL usage, not total host CPU or
RAM. The authenticated `/api/usage/history` endpoint returns fleet history;
`?instance_id=<UUID>` scopes it to one hosting server. Aggregation uses current
tenant placement and eligibility, so moving a tenant changes its history's scope.

Aggregate history takes the latest sample per tenant in each ten-second bucket.
Transaction rates are calculated per tenant before summing, and counter resets
or gaps longer than thirty seconds invalidate the rate. Database graphs show totals from reporting tenants, with coverage available
on hover or keyboard focus. Cards sum only readings no older than thirty seconds, show
partial coverage explicitly, and display unavailable metrics as missing rather
than zero. A missing retained database does not hide healthy tenants’ database
charts. The reporting count covers eligible, configured tenants.

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

Eunha runtime releases now support an optional, separate loopback-only
Prometheus `/metrics` listener. Blue/green slots enable it on their direct HTTP
port plus 1,000, with distinct targets for each process. The stable runtime
proxy and public tunnel do not route to it. It exports tenant-labelled request
counts, response-header latency histograms, in-flight and aborted requests,
admission rejections, and application database-pool connections.

## Service collection and charts

The control plane scrapes this Prometheus exposition privately, using its
existing operator SSH access to each hosting server. It does not run a separate
Prometheus server. The sampler reads the active blue/green slot and scrapes its
loopback listener; draining slots are deliberately excluded to avoid counting a
tenant twice. A slot switch during a scrape invalidates that reading. Requests
already draining on an old slot are outside this measurement.

Scrapes run independently of database collection, with up to four runtimes in
parallel, an eight-second overall deadline per runtime, a three-second HTTP
deadline, and an eight-MiB exposition limit. The collector waits ten seconds
between rounds, so large fleets or slow hosts increase the cadence. HTTP proxies
and redirects are disabled. Runtimes without blue/green routing, older binaries,
and unavailable listeners produce missing readings rather than zeroes.

Only known canonical tenant domains are accepted. Route labels and raw
exposition never enter storage, logs, or APIs. Every successful tenant reading
must include all three refreshed pool/in-flight gauges. Missing request counters
on an otherwise reporting tenant mean no activity; unknown tenants are ignored.
Retained tenants have no serving service and are excluded. Ready and failed
tenants remain eligible, with failures reflected in coverage.

Migration 044 stores numeric service samples and private process identities for
28 days. The operator-only `/api/usage/service/history` returns the selected range (ten minutes by default), with optional `tenant_id=<UUID>` or `instance_id=<UUID>` filters.
Scope uses current tenant placement. The API exposes numeric summaries and
coverage, never process identities or runtime paths. Reloading the page restores
charts from PostgreSQL.

Fleet, hosting-server, and tenant charts show requests per second, 5xx error
percentage, mean response-header latency, in-flight requests, total and idle
application pool connections, aborted requests per second, and capacity
rejections per second. Mean latency uses histogram sum/count deltas, weighted by
request volume; it is not a percentile. Latency excludes streamed body time.
Admission rejections have their own counter and do not enter request/error rates.
Monitoring requests are included in service traffic.

Counter rates require consecutive readings within thirty seconds, with the same
active slot, process PID, and process start time. Deployments, restarts, declining
counters (including idle-series expiration), failed scrapes, and longer pauses
produce gaps. Every tenant must report a metric for fleet/server totals to be
shown; partial coverage is explicit. Error percentage and latency are missing
when there is no traffic. Recent chart readings are marked stale when collection or history refresh falls
behind its expected cadence; the threshold accommodates longer-range refreshes.

## Shared time range

A single selector at the top of `/admin/usage` controls every database and service
chart across fleet, hosting-server, and tenant scopes. Presets cover ten minutes,
one or six hours, one or three days, and one, two, or four weeks. Current database cards
continue to show live readings regardless of the selected historical range.

All history APIs accept `minutes`, defaulting to ten minutes and bounded to
10–40,320 minutes. Both sample tables now retain 28 days. Older data previously
removed under the 24-hour policy cannot be recovered; week-long history builds
from collection going forward. No schema migration is needed for this change.

Queries compute counter rates from consecutive raw readings before aggregating
into coarser time buckets, returning at most approximately 360 points. Gauges and
rates are averaged within buckets; service error percentages and mean latencies
remain weighted by request counts. Buckets containing a missing metric or reset
retain a gap for that metric. Database totals continue to carry partial tenant
coverage. Charts use the selected window and date labels for multi-day ranges.
Ranges longer than an hour refresh historical graphs once per minute; shorter
ranges refresh every ten seconds. Live database cards retain ten-second refresh.
