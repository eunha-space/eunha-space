import { useEffect, useState } from "react";
import { api } from "@/api";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { UsageChart } from "./usage-chart";

export type UsageReading = {
  tenant_id: string; sampled_at: number; database_bytes: number | null;
  connections: number | null; active_connections: number | null; rate: number | null;
};
type AggregateHistory = {
  eligible_tenants: number;
  samples: { sampled_at: number; reporting_tenants: number; database_bytes: number | null;
    connections: number | null; active_connections: number | null; rate: number | null }[];
};
export function formatBytes(value: number) {
  const unit = value >= 1024 ** 3 ? 3 : value >= 1024 ** 2 ? 2 : value >= 1024 ? 1 : 0;
  return `${(value / 1024 ** unit).toLocaleString(undefined, { maximumFractionDigits: 1 })} ${["B", "KiB", "MiB", "GiB"][unit]}`;
}
const number = (value: number) => value.toLocaleString(undefined, { maximumFractionDigits: 1 });

export function AggregateUsage({ instanceId, readings, expected, now, loaded }: {
  instanceId?: string; readings: UsageReading[]; expected: number; now: number; loaded: boolean;
}) {
  const [history, setHistory] = useState<AggregateHistory>();
  const [error, setError] = useState(false);
  useEffect(() => {
    let disposed = false;
    let inFlight = false;
    async function load() {
      if (inFlight || document.hidden) return;
      inFlight = true;
      try {
        const result = await api<AggregateHistory>(`/usage/history${instanceId ? `?instance_id=${encodeURIComponent(instanceId)}` : ""}`);
        if (!disposed) { setHistory(result); setError(false); }
      } catch { if (!disposed) setError(true); }
      finally { inFlight = false; }
    }
    void load();
    const timer = setInterval(() => void load(), 10000);
    const visible = () => { if (!document.hidden) void load(); };
    document.addEventListener("visibilitychange", visible);
    return () => { disposed = true; clearInterval(timer); document.removeEventListener("visibilitychange", visible); };
  }, [instanceId]);
  const eligible = history?.eligible_tenants ?? expected;
  const fresh = readings.filter(row => now - row.sampled_at <= 30000);
  const reporting = fresh.filter(row => row.database_bytes !== null).length;
  const metrics = [
    { title: "Database storage", field: "database_bytes", format: formatBytes, color: "var(--chart-1)" },
    { title: "Database connections", field: "connections", format: number, color: "var(--chart-2)" },
    { title: "Active connections", field: "active_connections", format: number, color: "var(--chart-3)" },
    { title: "Transactions / second", field: "rate", format: number, color: "var(--chart-4)" },
  ] as const;
  return <div className="grid gap-3">
    <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-5">
      {metrics.map(metric => {
        const measured = fresh.filter(row => row[metric.field] !== null);
        const value = measured.reduce((sum, row) => sum + row[metric.field]!, 0);
        return <Card key={metric.field}><CardHeader><CardTitle>{metric.title}</CardTitle></CardHeader><CardContent>
          <div className="text-2xl font-semibold tabular-nums">{!loaded || (!measured.length && eligible > 0) ? "—" : metric.format(value)}</div>
          {loaded && measured.length < eligible && <Badge variant="outline" className="mt-2">Partial · {measured.length} / {eligible}</Badge>}
        </CardContent></Card>;
      })}
      <Card><CardHeader><CardTitle>Reporting tenants</CardTitle></CardHeader><CardContent className="text-2xl font-semibold tabular-nums">{loaded ? `${reporting} / ${eligible}` : "—"}</CardContent></Card>
    </div>
    {eligible === 0 && <p className="text-muted-foreground text-sm">No tenants eligible for sampling in this scope.</p>}
    {error && <p role="status" className="text-destructive text-sm">Aggregate history could not be refreshed. Previous graph readings may be stale.</p>}
    <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
      {metrics.map(metric => <UsageChart key={metric.field} title={metric.title} now={now}
        points={(history?.samples ?? []).map(row => ({ at: row.sampled_at, value: row[metric.field] }))}
        format={metric.format} color={metric.color} />)}
    </div>
    <p className="text-muted-foreground text-xs">Last 10 minutes, in 10-second buckets. Cards sum fresh reporting tenants and mark partial coverage. Graphs show gaps when any eligible tenant is missing a metric. Ready, failed, and retained tenants are included.</p>
  </div>;
}
