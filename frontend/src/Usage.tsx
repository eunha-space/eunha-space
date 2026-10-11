import { useEffect, useRef, useState } from "react";
import { api, errorMessage, type Tenant, type Runtime } from "@/api";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { UsageChart } from "@/components/console/usage-chart";
import { NativeSelect } from "@/components/ui/native-select";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { AggregateUsage, formatBytes as bytes, formatNumber } from "@/components/console/aggregate-usage";
import { ServiceUsage } from "@/components/console/service-usage";
import { pathFor, useSubroute } from "@/router";
import { cn } from "cn";

type Sample = { tenant_id: string; sampled_at: number; database_bytes: number | null; connections: number | null; active_connections: number | null; transactions: number | null; stats_reset: string | null; rate?: number | null };
type Reading = Sample & { rate: number | null };

const RANGES = [
  { value: 10, label: "Past 10 minutes" }, { value: 60, label: "Past hour" }, { value: 360, label: "Past 6 hours" },
  { value: 1440, label: "Past day" }, { value: 4320, label: "Past 3 days" }, { value: 10080, label: "Past week" },
  { value: 20160, label: "Past 2 weeks" }, { value: 40320, label: "Past 4 weeks" },
];
const PAGES = [
  { id: "fleet", label: "Fleet", path: [] },
  { id: "servers", label: "Hosting servers", path: ["servers"] },
  { id: "tenants", label: "Tenants", path: ["tenants"] },
] as const;
type Page = (typeof PAGES)[number]["id"];

const eligible = (tenant: Tenant) => ["ready", "failed", "retained"].includes(tenant.lifecycle_state);

/**
 * Fleet, hosting-server, and tenant usage are separate pages under
 * /admin/usage so only one scope's charts are fetched and drawn at a time.
 */
export function Usage({ tenants, runtimes }: { tenants: Tenant[]; runtimes: Runtime[] }) {
  const [segments, go] = useSubroute("usage");
  const page: Page = segments[0] === "servers" || segments[0] === "tenants" ? segments[0] : "fleet";
  const [minutes, setMinutes] = useState(60);
  const [readings, setReadings] = useState<Reading[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState("");
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

  return <div className="grid gap-4">
    <div className="flex flex-wrap items-end justify-between gap-3">
      <nav aria-label="Usage scope" className="bg-muted flex w-fit flex-wrap gap-1 rounded-lg p-1">
        {PAGES.map(item => <Button key={item.id} size="sm" variant="ghost"
          render={<a href={[pathFor("usage"), ...item.path].join("/")} />}
          aria-current={page === item.id ? "page" : undefined}
          className={cn(page === item.id ? "bg-background text-foreground shadow-sm hover:bg-background" : "text-muted-foreground")}
          onClick={event => {
            if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
            event.preventDefault();
            go([...item.path]);
          }}>{item.label}</Button>)}
      </nav>
      <label className="grid gap-1 text-sm">Time range
        <NativeSelect value={minutes} onChange={event => setMinutes(Number(event.target.value))}>
          {RANGES.map(range => <option key={range.value} value={range.value}>{range.label}</option>)}
        </NativeSelect>
      </label>
    </div>
    <p className="text-muted-foreground text-xs">Live database and Eunha service metrics, collected privately in the background about every 10 seconds. CPU, RAM, media storage, and bandwidth are not measured. Database connections reflect PostgreSQL backends; service pool charts show application connections. Long ranges use coarser chart samples, and history is retained for 28 days. Current cards always show live readings.</p>
    {error && <Alert variant="destructive"><AlertDescription>Usage refresh failed. Previous readings may be stale. {error}</AlertDescription></Alert>}
    {page === "fleet" && <FleetUsage tenants={tenants} readings={readings} minutes={minutes} now={now} loaded={loaded} />}
    {page === "servers" && <ServerUsage tenants={tenants} runtimes={runtimes} readings={readings} minutes={minutes} now={now} loaded={loaded}
      instanceId={segments[1]} onSelect={id => go(["servers", id])} />}
    {page === "tenants" && <TenantUsage tenants={tenants} readings={readings} minutes={minutes} now={now} loaded={loaded}
      tenantId={segments[1]} onSelect={id => go(["tenants", id])} />}
  </div>;
}

type Scope = { tenants: Tenant[]; readings: Reading[]; minutes: number; now: number; loaded: boolean };

function FleetUsage({ tenants, readings, minutes, now, loaded }: Scope) {
  const ids = new Set(tenants.filter(eligible).map(tenant => tenant.id));
  return <section className="grid gap-3" aria-labelledby="fleet-usage-title">
    <div><h2 id="fleet-usage-title" className="font-semibold">Fleet-wide usage</h2><p className="text-muted-foreground text-xs">Database and service usage across every hosting server and runtime.</p></div>
    <AggregateUsage minutes={minutes} readings={readings.filter(row => ids.has(row.tenant_id))} expected={ids.size} now={now} loaded={loaded} />
    <ServiceUsage minutes={minutes} now={now} />
  </section>;
}

function ServerUsage({ tenants, runtimes, readings, minutes, now, loaded, instanceId, onSelect }: Scope & { runtimes: Runtime[]; instanceId?: string; onSelect: (id: string) => void }) {
  const instances = [...new Map(runtimes.map(runtime => [runtime.instance_id, { id: runtime.instance_id, name: runtime.instance_name }])).values()].sort((a, b) => a.name.localeCompare(b.name));
  const instance = instances.find(row => row.id === instanceId) ?? instances[0];
  const runtimeNames = new Set(runtimes.filter(runtime => runtime.instance_id === instance?.id).map(runtime => runtime.name));
  const ids = new Set(tenants.filter(tenant => eligible(tenant) && runtimeNames.has(tenant.runtime_name)).map(tenant => tenant.id));
  return <section className="grid gap-3" aria-labelledby="instance-usage-title">
    <div className="flex flex-wrap items-end justify-between gap-3">
      <div><h2 id="instance-usage-title" className="font-semibold">Hosting server usage</h2><p className="text-muted-foreground text-xs">Combined tenant database and service usage for the selected hosting server.</p></div>
      <label className="grid gap-1 text-sm">Hosting server
        <NativeSelect value={instance?.id ?? ""} onChange={event => onSelect(event.target.value)} disabled={!instances.length}>
          {!instances.length && <option value="">No hosting servers</option>}
          {instances.map(row => <option key={row.id} value={row.id}>{row.name}</option>)}
        </NativeSelect>
      </label>
    </div>
    {instance ? <>
      <AggregateUsage minutes={minutes} key={instance.id} instanceId={instance.id}
        readings={readings.filter(row => ids.has(row.tenant_id))} expected={ids.size} now={now} loaded={loaded} />
      <ServiceUsage minutes={minutes} key={`service-${instance.id}`} instanceId={instance.id} now={now} />
    </> : <p className="text-muted-foreground text-sm">No hosting servers are registered.</p>}
  </section>;
}

function TenantUsage({ tenants, readings, minutes, now, loaded, tenantId, onSelect }: Scope & { tenantId?: string; onSelect: (id: string) => void }) {
  const byId = new Map(readings.map(sample => [sample.tenant_id, sample]));
  const largest = [...tenants].sort((a, b) => (byId.get(b.id)?.database_bytes ?? -1) - (byId.get(a.id)?.database_bytes ?? -1));
  const selected = tenants.find(tenant => tenant.id === tenantId) ?? largest[0];
  const heading = useRef<HTMLHeadingElement>(null);
  return <section className="grid gap-3" aria-labelledby="tenant-usage-title">
    <div className="flex flex-wrap items-end justify-between gap-3">
      <div><h2 id="tenant-usage-title" ref={heading} tabIndex={-1} className="font-semibold outline-none">Tenant usage</h2><p className="text-muted-foreground text-xs">Selected range, restored from stored samples after reload. Gaps indicate missing samples or pauses.</p></div>
      <label className="grid gap-1 text-sm">Tenant
        <NativeSelect value={selected?.id ?? ""} onChange={event => onSelect(event.target.value)} disabled={!tenants.length}>
          {!tenants.length && <option value="">No tenants</option>}
          {tenants.map(tenant => <option key={tenant.id} value={tenant.id}>{tenant.domain}</option>)}
        </NativeSelect>
      </label>
    </div>
    {selected && <>
      <TenantTrends key={selected.id} tenantId={selected.id} minutes={minutes} now={now} />
      <ServiceUsage minutes={minutes} key={`service-${selected.id}`} tenantId={selected.id} now={now} />
    </>}
    <TenantTable tenants={largest} byId={byId} loaded={loaded} now={now} onSelect={id => {
      onSelect(id);
      heading.current?.scrollIntoView({ behavior: "smooth", block: "start" });
      heading.current?.focus({ preventScroll: true });
    }} />
  </section>;
}

function TenantTrends({ tenantId, minutes, now }: { tenantId: string; minutes: number; now: number }) {
  const [trend, setTrend] = useState<Sample[]>([]);
  const [error, setError] = useState("");
  useEffect(() => {
    setTrend([]);
    setError("");
    let disposed = false;
    let inFlight = false;
    async function load() {
      if (inFlight || document.hidden) return;
      inFlight = true;
      try {
        const samples = await api<Sample[]>(`/tenants/${tenantId}/usage/history?minutes=${minutes}`);
        if (!disposed) { setTrend(samples); setError(""); }
      } catch { if (!disposed) setError("Stored history could not be refreshed. Previous readings may be stale."); }
      finally { inFlight = false; }
    }
    void load();
    const timer = setInterval(() => void load(), minutes > 60 ? 60000 : 10000);
    const visible = () => { if (!document.hidden) void load(); };
    document.addEventListener("visibilitychange", visible);
    return () => { disposed = true; clearInterval(timer); document.removeEventListener("visibilitychange", visible); };
  }, [tenantId, minutes]);
  const windowMs = minutes * 60000;
  return <div className="grid gap-3">
    <h3 className="text-sm font-semibold">PostgreSQL database</h3>
    {error && <p role="status" className="text-muted-foreground text-sm">{error}</p>}
    <div className="grid gap-3 xl:grid-cols-3">
      <UsageChart windowMs={windowMs} title="Database storage" now={now} points={trend.map(row => ({ at: row.sampled_at, value: row.database_bytes }))} format={bytes} />
      <UsageChart windowMs={windowMs} title="Database connections" now={now} points={trend.map(row => ({ at: row.sampled_at, value: row.connections }))} format={formatNumber} color="var(--chart-2)" />
      <UsageChart windowMs={windowMs} title="Transactions / second" now={now} points={trend.map(row => ({ at: row.sampled_at, value: row.rate ?? null }))} format={value => value.toFixed(1)} color="var(--chart-4)" />
    </div>
  </div>;
}

/** Owns the filter so typing redraws the table, not the charts. */
function TenantTable({ tenants, byId, loaded, now, onSelect }: { tenants: Tenant[]; byId: Map<string, Reading>; loaded: boolean; now: number; onSelect: (id: string) => void }) {
  const [search, setSearch] = useState("");
  const rows = tenants.filter(tenant => `${tenant.domain} ${tenant.runtime_name}`.toLowerCase().includes(search.toLowerCase()));
  return <div className="grid gap-3">
    <Input aria-label="Filter tenants" placeholder="Filter by tenant or runtime…" value={search} onChange={event => setSearch(event.target.value)} className="max-w-sm" />
    <Card><CardContent><Table><TableHeader><TableRow><TableHead>Tenant</TableHead><TableHead>Runtime</TableHead><TableHead>Database storage</TableHead><TableHead>Connections / active</TableHead><TableHead>Transactions / sec</TableHead><TableHead>Sample</TableHead></TableRow></TableHeader><TableBody>
      {rows.map(tenant => {
        const row = byId.get(tenant.id);
        const age = row ? Math.max(0, Math.floor((now - row.sampled_at) / 1000)) : 0;
        return <TableRow key={tenant.id}><TableCell><Button variant="link" className="h-auto p-0" onClick={() => onSelect(tenant.id)} aria-label={`Graph usage for ${tenant.domain}`}>{tenant.domain}</Button></TableCell><TableCell>{tenant.runtime_name}</TableCell><TableCell className="tabular-nums">{row?.database_bytes != null ? bytes(row.database_bytes) : "—"}</TableCell><TableCell className="tabular-nums">{row?.connections != null ? `${row.connections} / ${row.active_connections}` : "—"}</TableCell><TableCell className="tabular-nums">{row?.rate != null ? row.rate.toFixed(1) : "—"}</TableCell><TableCell><Badge variant="outline">{!loaded ? "Loading" : !row ? "Not sampled" : row.database_bytes === null ? "Unavailable" : age > 30 ? "Stale" : "Live"}</Badge>{row && <span className="text-muted-foreground ml-2 text-xs">{age}s ago</span>}</TableCell></TableRow>;
      })}
      {rows.length === 0 && <TableRow><TableCell colSpan={6} className="text-muted-foreground py-8 text-center">No tenants match.</TableCell></TableRow>}
    </TableBody></Table><p className="text-muted-foreground mt-3 text-xs">Sorted by database size. Transaction rates appear after two samples and reset when PostgreSQL statistics reset. Retained tenants still occupy storage; provisioning and purged tenants are not sampled.</p></CardContent></Card>
  </div>;
}
