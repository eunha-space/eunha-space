import { useEffect, useState } from "react";
import { api, errorMessage, type Tenant } from "@/api";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { UsageChart } from "@/components/console/usage-chart";
import { NativeSelect } from "@/components/ui/native-select";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription } from "@/components/ui/alert";

type Sample = { tenant_id: string; sampled_at: number; database_bytes: number | null; connections: number | null; active_connections: number | null; transactions: number | null; stats_reset: string | null };
type Reading = Sample & { rate: number | null };
function bytes(value: number) {
  const unit = value >= 1024 ** 3 ? 3 : value >= 1024 ** 2 ? 2 : value >= 1024 ? 1 : 0;
  return `${(value / 1024 ** unit).toLocaleString(undefined, { maximumFractionDigits: 1 })} ${["B", "KiB", "MiB", "GiB"][unit]}`;
}

export function Usage({ tenants }: { tenants: Tenant[] }) {
  const [readings, setReadings] = useState<Reading[]>([]);
  const [history, setHistory] = useState<Map<string, Reading[]>>(new Map());
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
          const valid = before && elapsed > 0 && sample.stats_reset === before.stats_reset && sample.transactions !== null && before.transactions !== null && sample.transactions >= before.transactions;
          const rate = valid ? (sample.transactions! - before!.transactions!) / elapsed : null;
          // Repeated cached readings preserve the last rate.
          if (elapsed === 0) return { ...sample, rate: before?.rate ?? null };
          return { ...sample, rate };
        });
        setReadings(next);
        setNow(Date.now());
        setHistory(current => {
          const cutoff = Date.now() - 10 * 60 * 1000;
          const updated = new Map<string, Reading[]>();
          for (const sample of next) {
            const points = (current.get(sample.tenant_id) ?? []).filter(point => point.sampled_at >= cutoff);
            if (!points.some(point => point.sampled_at === sample.sampled_at)) points.push(sample);
            updated.set(sample.tenant_id, points.slice(-120));
          }
          return updated;
        });
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
  const available = readings.filter(sample => sample.database_bytes !== null);
  const rows = tenants.filter(tenant => `${tenant.domain} ${tenant.title} ${tenant.runtime_name}`.toLowerCase().includes(search.toLowerCase())).sort((a, b) => (byId.get(b.id)?.database_bytes ?? -1) - (byId.get(a.id)?.database_bytes ?? -1));
  const selected = tenants.find(tenant => tenant.id === selectedId) ?? rows[0];
  const trend = selected ? history.get(selected.id) ?? [] : [];
  return <div className="grid gap-4">
    <p className="text-muted-foreground text-sm">Live PostgreSQL usage, sampled every 10 seconds while this page is visible. CPU, RAM, media storage, and bandwidth require additional runtime instrumentation. Connections reflect direct PostgreSQL backends; pooled client connections are not included.</p>
    {error && <Alert variant="destructive"><AlertDescription>Usage refresh failed. Previous readings may be stale. {error}</AlertDescription></Alert>}
    <div className="grid gap-3 sm:grid-cols-3">
      {[ ["Database storage", loaded ? bytes(available.reduce((sum, row) => sum + row.database_bytes!, 0)) : "—"], ["Database connections", loaded ? available.reduce((sum, row) => sum + (row.connections ?? 0), 0).toLocaleString() : "—"], ["Reporting tenants", loaded ? `${available.length} / ${tenants.length}` : "—"] ].map(([label, value]) => <Card key={label}><CardHeader><CardTitle>{label}</CardTitle></CardHeader><CardContent className="text-2xl font-semibold tabular-nums">{value}</CardContent></Card>)}
    </div>
    <div className="grid gap-3">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div><h2 className="font-semibold">Tenant trends</h2><p className="text-muted-foreground text-xs">Last 10 minutes collected on this page. History resets when you leave or reload. Gaps indicate missing samples or pauses.</p></div>
        <label className="grid gap-1 text-sm">Tenant
          <NativeSelect value={selected?.id ?? ""} onChange={event => setSelectedId(event.target.value)} disabled={!tenants.length}>
            {!tenants.length && <option value="">No tenants</option>}
            {tenants.map(tenant => <option key={tenant.id} value={tenant.id}>{tenant.domain}</option>)}
          </NativeSelect>
        </label>
      </div>
      <div className="grid gap-3 xl:grid-cols-3">
        <UsageChart title="Database storage" now={now} points={trend.map(row => ({ at: row.sampled_at, value: row.database_bytes }))} format={bytes} />
        <UsageChart title="Database connections" now={now} points={trend.map(row => ({ at: row.sampled_at, value: row.connections }))} format={value => value.toLocaleString(undefined, { maximumFractionDigits: 1 })} color="var(--chart-2)" />
        <UsageChart title="Transactions / second" now={now} points={trend.map(row => ({ at: row.sampled_at, value: row.rate }))} format={value => value.toFixed(1)} color="var(--chart-4)" />
      </div>
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
