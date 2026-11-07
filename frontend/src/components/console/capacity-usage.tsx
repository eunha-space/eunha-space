import { useEffect, useState } from "react";
import { api } from "@/api";
import { Card, CardContent } from "@/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";

type Capacity = {
  tenant_id: string; domain: string; lifecycle_state: string; media_exempt: boolean;
  plan: string | null; user_limit: number | null; media_gb: number | null; database_gb: number | null;
  monthly_activities: number | null; sampled_at: number | null; users: number | null;
  local_media_bytes: number | null; remote_media_bytes: number | null; database_bytes: number | null;
  monthly_deliveries: number | null; delivery_observed_ms: number | null;
};
const gb = 1_000_000_000;
const ratio = (value: number | null, limit: number | null) => value === null || limit === null ? null : limit === 0 ? (value > 0 ? Infinity : 0) : value / limit;
const pressure = (r: Capacity, now: number) => !r.sampled_at || now - r.sampled_at > 1_800_000 ? -1 : Math.max(-1,
  ratio(r.users, r.user_limit) ?? -1,
  r.media_exempt ? -1 : ratio(r.local_media_bytes, r.media_gb === null ? null : r.media_gb * gb) ?? -1,
  ratio(r.database_bytes, r.database_gb === null ? null : r.database_gb * gb) ?? -1);
function amount(value: number | null, limit: number | null, storage = false) {
  if (value === null) return "Unavailable";
  const format = storage ? (n: number) => {
    const unit = n >= 1e9 ? 3 : n >= 1e6 ? 2 : n >= 1e3 ? 1 : 0;
    return `${(n / 1000 ** unit).toLocaleString(undefined, { maximumFractionDigits: 2 })} ${["B", "KB", "MB", "GB"][unit]}`;
  } : (n: number) => n.toLocaleString();
  return `${format(value)}${limit === null ? "" : ` / ${format(limit)}`}`;
}

export function CapacityUsage({ tenantId, now }: { tenantId?: string; now: number }) {
  const [rows, setRows] = useState<Capacity[]>([]);
  const [error, setError] = useState(false);
  const [loaded, setLoaded] = useState(false);
  useEffect(() => {
    let disposed = false;
    let busy = false;
    async function load() {
      if (busy || document.hidden) return;
      busy = true;
      try { const result = await api<Capacity[]>("/usage/capacity"); if (!disposed) { setRows(result); setError(false); setLoaded(true); } }
      catch { if (!disposed) setError(true); }
      finally { busy = false; }
    }
    void load();
    const timer = setInterval(() => void load(), 60000);
    return () => { disposed = true; clearInterval(timer); };
  }, []);
  const selected = rows.filter(row => !tenantId || row.tenant_id === tenantId).sort((a, b) => pressure(b, now) - pressure(a, now) || a.domain.localeCompare(b.domain));
  return <Card><CardContent className="grid gap-3">
    <h3 className="font-semibold">Instance capacity · measurement only</h3>
    <p className="text-muted-foreground text-xs">Collected every 15 minutes. Sorted by highest capacity percentage of plan used. Media counts recorded originals and thumbnails, profiles and emoji; resized variants are not included. Remote cache is shown separately. No limits are enforced.</p>
    {error && <p role="status" className="text-destructive text-sm">Capacity refresh failed. Previous readings may be stale.</p>}
    <Table><TableHeader><TableRow><TableHead>Tenant / plan</TableHead><TableHead>Plan used</TableHead><TableHead>Users</TableHead><TableHead>Local media</TableHead><TableHead>Remote cache</TableHead><TableHead>Database</TableHead><TableHead>Deliveries this month (UTC)</TableHead><TableHead>Measured</TableHead></TableRow></TableHeader><TableBody>
      {selected.map(row => <TableRow key={row.tenant_id}>
        <TableCell>{row.domain}<div className="text-muted-foreground text-xs">{row.lifecycle_state === "retained" ? "Retained · storage only" : row.plan ?? "No plan"}</div></TableCell>
        <TableCell>{pressure(row, now) < 0 ? "Unavailable" : pressure(row, now) === Infinity ? "Over zero allowance" : `${Math.round(pressure(row, now) * 100)}%`}</TableCell>
        <TableCell>{amount(row.users, row.user_limit)}</TableCell>
        <TableCell>{amount(row.local_media_bytes, row.media_exempt ? null : row.media_gb === null ? null : row.media_gb * gb, true)}{row.media_exempt && <div className="text-muted-foreground text-xs">Customer storage · excluded</div>}</TableCell>
        <TableCell>{amount(row.remote_media_bytes, null, true)}</TableCell>
        <TableCell>{amount(row.database_bytes, row.database_gb === null ? null : row.database_gb * gb, true)}</TableCell>
        <TableCell>{row.monthly_deliveries === null ? "Unavailable" : `${amount(row.monthly_deliveries, row.monthly_activities)} observed`}<div className="text-muted-foreground text-xs">{row.delivery_observed_ms === null ? "" : `${(row.delivery_observed_ms / 3600000).toFixed(1)} hours covered · `}Fair use</div></TableCell>
        <TableCell>{row.sampled_at === null ? "Not sampled" : <>{new Date(row.sampled_at).toLocaleString()}{now - row.sampled_at > 1_800_000 && <div className="text-destructive text-xs">Stale</div>}</>}</TableCell>
      </TableRow>)}
      {!selected.length && <TableRow><TableCell colSpan={8}>{loaded ? "No capacity readings." : "Loading…"}</TableCell></TableRow>}
    </TableBody></Table>
    <p className="text-muted-foreground text-xs">Daily capacity history is retained for a year. Delivery counts are observed successful sends, including redeliveries, rather than unique posts. Scrape gaps, restarts and intervals crossing UTC midnight are excluded; these are lower bounds, not billing totals.</p>
  </CardContent></Card>;
}

type Daily = { sampled_at: number; users: number | null; local_media_bytes: number | null; remote_media_bytes: number | null; database_bytes: number | null };
export function CapacityHistory({ tenantId }: { tenantId: string }) {
  const [rows, setRows] = useState<Daily[]>([]);
  const [error, setError] = useState(false);
  useEffect(() => {
    let disposed = false;
    async function load() {
      try { const result = await api<Daily[]>(`/tenants/${tenantId}/usage/capacity`); if (!disposed) { setRows(result); setError(false); } }
      catch { if (!disposed) setError(true); }
    }
    void load();
    const timer = setInterval(() => void load(), 900000);
    return () => { disposed = true; clearInterval(timer); };
  }, [tenantId]);
  return <Card><CardContent className="grid gap-3">
    <h3 className="text-sm font-semibold">Daily capacity history</h3>
    <p className="text-muted-foreground text-xs">Latest reading per UTC day, retained for a year. Missing readings stay unavailable.</p>
    {error && <p role="status" className="text-destructive text-sm">Daily history could not be refreshed.</p>}
    <div className="max-h-72 overflow-auto"><Table><TableHeader><TableRow><TableHead>Measured</TableHead><TableHead>Users</TableHead><TableHead>Local media</TableHead><TableHead>Remote cache</TableHead><TableHead>Database</TableHead></TableRow></TableHeader><TableBody>
      {[...rows].reverse().map(row => <TableRow key={row.sampled_at}><TableCell>{new Date(row.sampled_at).toLocaleString()}</TableCell><TableCell>{amount(row.users, null)}</TableCell><TableCell>{amount(row.local_media_bytes, null, true)}</TableCell><TableCell>{amount(row.remote_media_bytes, null, true)}</TableCell><TableCell>{amount(row.database_bytes, null, true)}</TableCell></TableRow>)}
      {!rows.length && <TableRow><TableCell colSpan={5}>No stored daily readings.</TableCell></TableRow>}
    </TableBody></Table></div>
  </CardContent></Card>;
}
