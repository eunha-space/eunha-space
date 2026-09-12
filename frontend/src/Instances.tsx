import { useMemo, useState } from "react";
import { HardDrive, Inbox, RefreshCw } from "lucide-react";
import { pendingRender, type Runtime, type Tenant } from "@/api";
import { absoluteTime, relativeTime } from "@/format";
import { Badge } from "@/components/ui/badge";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import { ActionButton } from "@/components/console/action-button";
import { ChipFilter, SearchField } from "@/components/console/filters";
import { EmptyState, StatusDot } from "@/components/console/status";
import { TenantConfigDialog } from "@/components/console/tenant-config";
import { cn } from "cn";

type Props = {
  tenants: Tenant[];
  runtimes: Runtime[];
  busy: string;
  act: (path: string, key: string, body?: object) => void;
};

const NEEDS_ATTENTION = new Set(["unhealthy", "unknown"]);

function matches(tenant: Tenant, filter: string): boolean {
  if (filter === "healthy") return tenant.observed_state === "healthy";
  if (filter === "attention") return NEEDS_ATTENTION.has(tenant.observed_state);
  if (filter === "pending") return pendingRender(tenant);
  return true;
}

export function Instances({ tenants, runtimes, busy, act }: Props) {
  const [filter, setFilter] = useState("all");
  const [query, setQuery] = useState("");

  const hostByRuntime = useMemo(
    () => new Map(runtimes.map((runtime) => [runtime.name, runtime.host])),
    [runtimes],
  );
  const hostCount = useMemo(() => new Set(runtimes.map((r) => r.host)).size, [runtimes]);

  // Grouping only earns its headers once the fleet spans more than one machine.
  const [grouped, setGrouped] = useState<boolean | null>(null);
  const groupByServer = grouped ?? hostCount > 1;

  const counts = useMemo(
    () => ({
      all: tenants.length,
      healthy: tenants.filter((t) => t.observed_state === "healthy").length,
      attention: tenants.filter((t) => NEEDS_ATTENTION.has(t.observed_state)).length,
      pending: tenants.filter(pendingRender).length,
    }),
    [tenants],
  );

  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return tenants.filter(
      (tenant) =>
        matches(tenant, filter) &&
        (needle === "" ||
          `${tenant.domain} ${tenant.title} ${tenant.runtime_name} ${hostByRuntime.get(tenant.runtime_name) ?? ""}`
            .toLowerCase()
            .includes(needle)),
    );
  }, [tenants, filter, query, hostByRuntime]);

  const groups = useMemo(() => {
    if (!groupByServer) return [["", visible]] as [string, Tenant[]][];
    const byHost = new Map<string, Tenant[]>();
    for (const tenant of visible) {
      const host = hostByRuntime.get(tenant.runtime_name) ?? "unknown host";
      const existing = byHost.get(host);
      if (existing) existing.push(tenant);
      else byHost.set(host, [tenant]);
    }
    return [...byHost.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [visible, groupByServer, hostByRuntime]);

  return (
    <div className="grid gap-4">
      <Card className="grid grid-cols-2 gap-0 overflow-hidden p-0 md:grid-cols-4">
        {(
          [
            ["all", "Managed"],
            ["healthy", "Healthy"],
            ["attention", "Needs attention"],
            ["pending", "Pending render"],
          ] as const
        ).map(([key, label], index) => (
          <button
            type="button"
            key={key}
            aria-pressed={filter === key}
            onClick={() => setFilter(key)}
            className={cn(
              "hover:bg-muted/60 focus-visible:ring-ring/50 cursor-pointer p-5 text-left transition-colors focus-visible:ring-3 focus-visible:outline-none",
              "border-border border-b md:border-b-0",
              index % 2 === 0 && "border-r",
              (index === 1 || index === 2) && "md:border-r",
              filter === key && "bg-accent/60 shadow-[inset_0_2px_0_var(--primary)]",
            )}
          >
            <span className="text-muted-foreground block text-sm">{label}</span>
            <strong
              className={cn(
                "mt-0.5 block text-3xl font-semibold tabular-nums",
                key === "attention" && counts.attention > 0 && "text-destructive",
              )}
            >
              {counts[key]}
            </strong>
          </button>
        ))}
      </Card>

      <Card className="gap-0 overflow-hidden p-0">
        <CardHeader className="flex flex-col gap-4 p-5 xl:flex-row xl:items-start xl:justify-between">
          <div>
            <CardTitle>Tenant inventory</CardTitle>
            <CardDescription>
              Showing {visible.length} of {tenants.length} tenants.
            </CardDescription>
          </div>
          <div className="flex flex-wrap items-center gap-3">
            <div className="flex items-center gap-2">
              <Switch
                id="group-by-server"
                checked={groupByServer}
                onCheckedChange={(next) => setGrouped(next)}
              />
              <Label htmlFor="group-by-server" className="text-muted-foreground whitespace-nowrap">
                Group by server
              </Label>
            </div>
            <SearchField
              value={query}
              onChange={setQuery}
              label="Search tenants"
              placeholder="Search domain, runtime, or host"
            />
            <ChipFilter
              label="Filter tenants by state"
              value={filter}
              onChange={setFilter}
              choices={[
                { value: "all", label: "All", count: counts.all },
                { value: "healthy", label: "Healthy", count: counts.healthy },
                { value: "attention", label: "Needs attention", count: counts.attention },
                { value: "pending", label: "Pending render", count: counts.pending },
              ]}
            />
          </div>
        </CardHeader>
        <Separator />
        {visible.length === 0 ? (
          <EmptyState icon={<Inbox />}>
            {tenants.length === 0
              ? "No tenants are managed by this control plane yet."
              : "No tenants match the current search and filter."}
          </EmptyState>
        ) : (
          groups.map(([host, hostTenants]) => (
            <section key={host || "all"}>
              {groupByServer && (
                <div className="bg-muted/50 border-border flex items-center gap-2 border-y px-5 py-2 first:border-t-0">
                  <HardDrive className="text-muted-foreground size-3.5" />
                  <h3 className="font-mono text-xs font-semibold">{host}</h3>
                  <span className="text-muted-foreground text-xs">
                    {hostTenants.length} tenant{hostTenants.length === 1 ? "" : "s"}
                  </span>
                </div>
              )}
              {hostTenants.map((tenant, index) => (
                <div
                  key={tenant.id}
                  className={cn(
                    "hover:bg-muted/40 grid items-center gap-4 p-5 transition-colors",
                    "grid-cols-[auto_1fr] xl:grid-cols-[auto_minmax(12rem,1fr)_minmax(18rem,1.2fr)_auto]",
                    index > 0 && "border-border border-t",
                  )}
                >
                  <StatusDot state={tenant.observed_state} />
                  <div className="min-w-0">
                    <h3 className="font-semibold">{tenant.domain}</h3>
                    <p className="text-muted-foreground mt-0.5 text-sm">{tenant.title}</p>
                    <div className="mt-1.5 flex flex-wrap gap-1.5 empty:hidden">
                      {pendingRender(tenant) && (
                        <Badge
                          variant="outline"
                          className="border-pending/40 bg-pending/10 text-pending"
                        >
                          Pending render
                        </Badge>
                      )}
                      {tenant.desired_state !== "running" && (
                        <Badge variant="outline" className="text-muted-foreground">
                          Desired: {tenant.desired_state}
                        </Badge>
                      )}
                    </div>
                  </div>
                  <dl className="col-span-2 grid grid-cols-2 gap-4 sm:grid-cols-3 xl:col-span-1">
                    <div className="min-w-0">
                      <dt className="text-muted-foreground text-xs tracking-[0.08em] uppercase">
                        Runtime
                      </dt>
                      <dd className="mt-0.5 text-sm break-words">
                        {tenant.runtime_name}
                        <span className="text-muted-foreground"> · {tenant.bind_address}</span>
                      </dd>
                    </div>
                    <div className="min-w-0">
                      <dt className="text-muted-foreground text-xs tracking-[0.08em] uppercase">
                        Config
                      </dt>
                      <dd className="mt-0.5 text-sm break-words">
                        revision {tenant.config_revision}
                        {pendingRender(tenant) && (
                          <span className="text-pending">
                            {" "}
                            · rendered {tenant.rendered_revision ?? "—"}
                          </span>
                        )}
                      </dd>
                    </div>
                    <div className="min-w-0">
                      <dt className="text-muted-foreground text-xs tracking-[0.08em] uppercase">
                        State
                      </dt>
                      <dd className="mt-0.5 text-sm break-words">
                        {tenant.observed_state}
                        <span
                          className="text-muted-foreground"
                          title={absoluteTime(tenant.updated_at)}
                        >
                          {" "}
                          · {relativeTime(tenant.updated_at)}
                        </span>
                      </dd>
                    </div>
                  </dl>
                  <div className="col-span-2 flex flex-wrap gap-2 xl:col-span-1 xl:justify-end">
                    <TenantConfigDialog tenantId={tenant.id} domain={tenant.domain} />
                    <ActionButton
                      icon={<RefreshCw />}
                      busy={busy === `check-${tenant.id}`}
                      onClick={() => act(`/tenants/${tenant.id}/check`, `check-${tenant.id}`)}
                    >
                      Check
                    </ActionButton>
                    <ActionButton
                      variant="default"
                      busy={busy === `render-${tenant.id}`}
                      confirm={{
                        title: `Render and restart ${tenant.domain}?`,
                        description: `This rewrites the runtime configuration file for ${tenant.domain} from revision ${tenant.config_revision} and restarts its service. The tenant is briefly unavailable while it restarts.`,
                        action: "Render & restart",
                      }}
                      onClick={() => act(`/tenants/${tenant.id}/render`, `render-${tenant.id}`)}
                    >
                      Render &amp; restart
                    </ActionButton>
                  </div>
                </div>
              ))}
            </section>
          ))
        )}
      </Card>
    </div>
  );
}
