import { useEffect, useState } from "react";
import { api } from "@/api";
import { Badge } from "@/components/ui/badge";
import { UsageChart } from "./usage-chart";

type ServicePoint = {
  sampled_at: number; reporting_tenants: number;
  request_rate: number | null; error_percent: number | null; latency_ms: number | null;
  aborted_rate: number | null; rejected_rate: number | null;
  in_flight: number | null; pool: number | null; pool_idle: number | null;
};
type History = { eligible_tenants: number; samples: ServicePoint[] };
const twoDecimals = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 });
const number = (value: number) => twoDecimals.format(value);
const metrics = [
  { field: "request_rate", title: "Service requests / second", format: number },
  { field: "error_percent", title: "Service server errors (5xx)", format: (value: number) => `${number(value)}%` },
  { field: "latency_ms", title: "Mean response-header latency", format: (value: number) => `${number(value)} ms` },
  { field: "in_flight", title: "Service requests in flight", format: number },
  { field: "pool", title: "Application pool connections", format: number },
  { field: "pool_idle", title: "Idle application pool connections", format: number },
  { field: "aborted_rate", title: "Aborted requests / second", format: number },
  { field: "rejected_rate", title: "Capacity rejections / second", format: number },
] as const;

export function ServiceUsage({ instanceId, tenantId, now, minutes }: { instanceId?: string; tenantId?: string; now: number; minutes: number }) {
  const [history, setHistory] = useState<History>();
  const [error, setError] = useState(false);
  useEffect(() => {
    let disposed = false;
    let inFlight = false;
    setHistory(undefined);
    setError(false);
    async function load() {
      if (inFlight || document.hidden) return;
      inFlight = true;
      const query = new URLSearchParams({ minutes: String(minutes) });
      if (instanceId) query.set("instance_id", instanceId);
      if (tenantId) query.set("tenant_id", tenantId);
      try {
        const result = await api<History>(`/usage/service/history?${query}`);
        if (!disposed) { setHistory(result); setError(false); }
      } catch { if (!disposed) setError(true); }
      finally { inFlight = false; }
    }
    void load();
    const timer = setInterval(() => void load(), minutes > 60 ? 60000 : 10000);
    const visible = () => { if (!document.hidden) void load(); };
    document.addEventListener("visibilitychange", visible);
    return () => { disposed = true; clearInterval(timer); document.removeEventListener("visibilitychange", visible); };
  }, [instanceId, tenantId, minutes]);
  const latest = history?.samples.at(-1);
  const fresh = latest && now - latest.sampled_at <= (minutes > 60 ? 90000 : 30000);
  return <div className="grid gap-3">
    <div className="flex flex-wrap items-center gap-2">
      <h3 className="text-sm font-semibold">Eunha service</h3>
      <Badge variant="outline">{!history ? "Loading" : history.eligible_tenants === 0 ? "No serving tenants" : !latest ? "Not sampled" : !fresh ? "Stale" : latest.reporting_tenants < history.eligible_tenants ? "Partial" : minutes > 60 ? "Recent" : "Live"}</Badge>
      {history && <span className="text-muted-foreground text-xs">Reporting: {fresh ? latest.reporting_tenants : 0} / {history.eligible_tenants} tenants{latest ? ` · ${Math.max(0, Math.floor((now - latest.sampled_at) / 1000))}s ago` : ""}</span>}
    </div>
    {error && <p role="status" className="text-destructive text-sm">Service metrics could not be refreshed. Previous readings may be stale.</p>}
    <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
      {metrics.map((metric, index) => <div key={metric.field} className="grid gap-1">
        <UsageChart windowMs={minutes * 60000} title={metric.title} now={now}
          points={(history?.samples ?? []).map(row => ({ at: row.sampled_at, value: row[metric.field] }))}
          format={metric.format} color={`var(--chart-${index % 4 + 1})`} />
        <p className="text-muted-foreground text-xs tabular-nums">Latest chart bucket: {fresh && latest[metric.field] !== null ? metric.format(latest[metric.field]!) : "—"}</p>
      </div>)}
    </div>
    <p className="text-muted-foreground text-xs">Private active-slot scrapes, approximately every 10 seconds. Graphs require complete coverage. Rates need two samples; process changes, counter resets and failed scrapes produce gaps. Latency measures response headers, excludes streamed bodies, and is weighted by requests. Error percentage and latency are unavailable when there is no traffic. Retained tenants are excluded.</p>
  </div>;
}
