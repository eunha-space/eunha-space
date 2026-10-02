import { useEffect, useState } from "react";
import { api, errorMessage, type Tenant, type Runtime } from "@/api";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { UsageChart } from "@/components/console/usage-chart";
import { NativeSelect } from "@/components/ui/native-select";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { AggregateUsage, formatBytes as bytes } from "@/components/console/aggregate-usage";
import { ServiceUsage } from "@/components/console/service-usage";

type Sample = { tenant_id: string; sampled_at: number; database_bytes: number | null; connections: number | null; active_connections: number | null; transactions: number | null; stats_reset: string | null; rate?: number | null };
type Reading = Sample & { rate: number | null };

export function Usage({ tenants, runtimes }: { tenants: Tenant[]; runtimes: Runtime[] }) {
  const [minutes, setMinutes] = useState(60);
  const windowMs = minutes * 60000;
  const [selectedInstanceId, setSelectedInstanceId] = useState("");
  const instances = [...new Map(runtimes.map(runtime => [runtime.instance_id, { id: runtime.instance_id, name: runtime.instance_name }])).values()].sort((a, b) => a.name.localeCompare(b.name));
  const instance = instances.find(row => row.id === selectedInstanceId) ?? instances[0];
  const instanceRuntimeNames = new Set(runtimes.filter(runtime => runtime.instance_id === instance?.id).map(runtime => runtime.name));
  const instanceTenants = tenants.filter(tenant => instanceRuntimeNames.has(tenant.runtime_name));
  const [readings, setReadings] = useState<Reading[]>([]);
  const [storedTrend, setStoredTrend] = useState<Sample[]>([]);
  const [selectedId, setSelectedId] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState("");
  const [search, setSearch] = useState("");
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    let disposed = false;
    let inFlight = false;
    let previous = new Map<string, Reading>();
    async function load() {
      if (inFlight || document.hidden) return;
      inFlight = true;
      try {
        const samples = await api<Sample[]>("/usage");
        if (disposed) return;
        const next = samples.map(sample => {
          const before = previous.get(sample.tenant_id);
          const elapsed = before ? (sample.sampled_at - before.sampled_at) / 1000 : 0;
          const valid = before && elapsed > 0 && elapsed <= 30 && sample.stats_reset === before.stats_reset && sample.transactions !== null && before.transactions !== null && sample.transactions >= before.transactions;
          const rate = valid ? (sample.transactions! - before!.transactions!) / elapsed : null;
          // Repeated cached readings preserve the last rate.
          if (elapsed === 0) return { ...sample, rate: before?.rate ?? null };
          return { ...sample, rate };
        });
        setReadings(next);
        setNow(Date.now());
        previous = new Map(next.map(sample => [sample.tenant_id, sample]));
        setLoaded(true);
        setError("");
      } catch (failure) {
        if (!disposed) setError(errorMessage(failure));
      } finally { inFlight = false; }
    }
    void load();
    const timer = setInterval(() => { setNow(Date.now()); void load(); }, 10000);
    const visible = () => { if (!document.hidden) void load(); };
    document.addEventListener("visibilitychange", visible);
    return () => { disposed = true; clearInterval(timer); document.removeEventListener("visibilitychange", visible); };
  }, []);
  const byId = new Map(readings.map(sample => [sample.tenant_id, sample]));
  const eligible = (tenant: Tenant) => ["ready", "failed", "retained"].includes(tenant.lifecycle_state);
  const eligibleIds = new Set(tenants.filter(eligible).map(tenant => tenant.id));
  const instanceEligibleIds = new Set(instanceTenants.filter(eligible).map(tenant => tenant.id));
  const rows = tenants.filter(tenant => `${tenant.domain} ${tenant.title} ${tenant.runtime_name}`.toLowerCase().includes(search.toLowerCase())).sort((a, b) => (byId.get(b.id)?.database_bytes ?? -1) - (byId.get(a.id)?.database_bytes ?? -1));
  const selected = tenants.find(tenant => tenant.id === selectedId) ?? rows[0];
  const selectedTenantId = selected?.id;
  const [historyError, setHistoryError] = useState("");
  useEffect(() => {
    setStoredTrend([]);
    setHistoryError("");
    if (!selectedTenantId) return;
    let disposed = false;
    let inFlight = false;
    async function load() {
      if (inFlight || document.hidden) return;
      inFlight = true;
      try {
        const samples = await api<Sample[]>(`/tenants/${selectedTenantId}/usage/history?minutes=${minutes}`);
        if (!disposed) { setStoredTrend(samples); setHistoryError(""); }
      } catch { if (!disposed) setHistoryError("Stored history could not be refreshed. Previous readings may be stale."); }
      finally { inFlight = false; }
    }
    void load();
    const timer = setInterval(() => void load(), minutes > 60 ? 60000 : 10000);
    const visible = () => { if (!document.hidden) void load(); };
    document.addEventListener("visibilitychange", visible);
    return () => { disposed = true; clearInterval(timer); document.removeEventListener("visibilitychange", visible); };
  }, [selectedTenantId, minutes]);
  const trend = storedTrend.map(row => ({ ...row, rate: row.rate ?? null }));
  return <div className="grid gap-4">
    <label className="grid max-w-xs gap-1 text-sm">Time range · all charts
      <NativeSelect value={minutes} onChange={event => setMinutes(Number(event.target.value))}>
        {[{ value: 10, label: "Past 10 minutes" }, { value: 60, label: "Past hour" }, { value: 360, label: "Past 6 hours" }, { value: 1440, label: "Past day" }, { value: 4320, label: "Past 3 days" }, { value: 10080, label: "Past week" }, { value: 20160, label: "Past 2 weeks" }, { value: 40320, label: "Past 4 weeks" }].map(range => <option key={range.value} value={range.value}>{range.label}</option>)}
      </NativeSelect>
    </label>
    <p className="text-muted-foreground text-xs">Long ranges use coarser chart samples. History is retained for 28 days from collection; previously expired data cannot be recovered. Current cards always show live readings.</p>
    <p className="text-muted-foreground text-sm">Live database and Eunha service metrics, collected privately in the background about every 10 seconds. CPU, RAM, media storage, and bandwidth are not measured. Database connections reflect PostgreSQL backends; service pool charts show application connections.</p>
    {error && <Alert variant="destructive"><AlertDescription>Usage refresh failed. Previous readings may be stale. {error}</AlertDescription></Alert>}
    <section className="grid gap-3" aria-labelledby="fleet-usage-title">
      <div><h2 id="fleet-usage-title" className="font-semibold">Fleet-wide usage</h2><p className="text-muted-foreground text-xs">Database and service usage across every hosting server and runtime.</p></div>
      <AggregateUsage minutes={minutes} readings={readings.filter(row => eligibleIds.has(row.tenant_id))} expected={tenants.filter(eligible).length} now={now} loaded={loaded} />
      <ServiceUsage minutes={minutes} now={now} />
    </section>
    <section className="grid gap-3" aria-labelledby="instance-usage-title">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div><h2 id="instance-usage-title" className="font-semibold">Instance-wide usage</h2><p className="text-muted-foreground text-xs">Combined tenant database and service usage for the selected hosting server.</p></div>
        <label className="grid gap-1 text-sm">Hosting server
          <NativeSelect value={instance?.id ?? ""} onChange={event => setSelectedInstanceId(event.target.value)} disabled={!instances.length}>
            {!instances.length && <option value="">No hosting servers</option>}
            {instances.map(row => <option key={row.id} value={row.id}>{row.name}</option>)}
          </NativeSelect>
        </label>
      </div>
      {instance ? <AggregateUsage minutes={minutes} key={instance.id} instanceId={instance.id}
        readings={readings.filter(row => instanceEligibleIds.has(row.tenant_id))}
        expected={instanceTenants.filter(eligible).length} now={now} loaded={loaded} />
        : <p className="text-muted-foreground text-sm">No hosting servers are registered.</p>}
      {instance && <ServiceUsage minutes={minutes} key={`service-${instance.id}`} instanceId={instance.id} now={now} />}
    </section>
    <div className="grid gap-3">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div><h2 className="font-semibold">Tenant trends</h2><p className="text-muted-foreground text-xs">Selected range, restored from stored samples after reload. Gaps indicate missing samples or pauses.</p></div>
        <label className="grid gap-1 text-sm">Tenant
          <NativeSelect value={selected?.id ?? ""} onChange={event => setSelectedId(event.target.value)} disabled={!tenants.length}>
            {!tenants.length && <option value="">No tenants</option>}
            {tenants.map(tenant => <option key={tenant.id} value={tenant.id}>{tenant.domain}</option>)}
          </NativeSelect>
        </label>
      </div>
      {historyError && <p role="status" className="text-muted-foreground text-sm">{historyError}</p>}
      <div className="grid gap-3 xl:grid-cols-3">
        <UsageChart windowMs={windowMs} title="Database storage" now={now} points={trend.map(row => ({ at: row.sampled_at, value: row.database_bytes }))} format={bytes} />
        <UsageChart windowMs={windowMs} title="Database connections" now={now} points={trend.map(row => ({ at: row.sampled_at, value: row.connections }))} format={value => value.toLocaleString(undefined, { maximumFractionDigits: 1 })} color="var(--chart-2)" />
        <UsageChart windowMs={windowMs} title="Transactions / second" now={now} points={trend.map(row => ({ at: row.sampled_at, value: row.rate }))} format={value => value.toFixed(1)} color="var(--chart-4)" />
      </div>
      {selected && <ServiceUsage minutes={minutes} key={`service-${selected.id}`} tenantId={selected.id} now={now} />}
    </div>
    <Input aria-label="Filter tenants" placeholder="Filter by tenant or runtime…" value={search} onChange={event => setSearch(event.target.value)} className="max-w-sm" />
    <Card><CardContent><Table><TableHeader><TableRow><TableHead>Tenant</TableHead><TableHead>Runtime</TableHead><TableHead>Database storage</TableHead><TableHead>Connections / active</TableHead><TableHead>Transactions / sec</TableHead><TableHead>Sample</TableHead></TableRow></TableHeader><TableBody>
      {rows.map(tenant => {
        const row = byId.get(tenant.id);
        const age = row ? Math.max(0, Math.floor((now - row.sampled_at) / 1000)) : 0;
        return <TableRow key={tenant.id}><TableCell><Button variant="link" className="h-auto p-0" onClick={() => setSelectedId(tenant.id)} aria-label={`Graph usage for ${tenant.domain}`}>{tenant.domain}</Button><div className="text-muted-foreground text-xs">{tenant.title}</div></TableCell><TableCell>{tenant.runtime_name}</TableCell><TableCell className="tabular-nums">{row?.database_bytes != null ? bytes(row.database_bytes) : "—"}</TableCell><TableCell className="tabular-nums">{row?.connections != null ? `${row.connections} / ${row.active_connections}` : "—"}</TableCell><TableCell className="tabular-nums">{row?.rate != null ? row.rate.toFixed(1) : "—"}</TableCell><TableCell><Badge variant="outline">{!loaded ? "Loading" : !row ? "Not sampled" : row.database_bytes === null ? "Unavailable" : age > 30 ? "Stale" : "Live"}</Badge>{row && <span className="text-muted-foreground ml-2 text-xs">{age}s ago</span>}</TableCell></TableRow>;
      })}
      {rows.length === 0 && <TableRow><TableCell colSpan={6} className="text-muted-foreground py-8 text-center">No tenants match.</TableCell></TableRow>}
    </TableBody></Table><p className="text-muted-foreground mt-3 text-xs">Sorted by database size. Totals include reporting tenants only. Transaction rates appear after two samples and reset when PostgreSQL statistics reset. Retained tenants still occupy storage; provisioning and purged tenants are not sampled.</p></CardContent></Card>
  </div>;
}
