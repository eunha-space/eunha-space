import { useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Inbox, RefreshCw, Rocket, Server } from "lucide-react";
import { isSettled, pendingRender, type Runtime, type Tenant } from "@/api";
import { absoluteTime, relativeTime, shortRef } from "@/format";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import { ActionButton } from "@/components/console/action-button";
import { ChipFilter, SearchField } from "@/components/console/filters";
import { OperationLog } from "@/components/console/operation-log";
import { EmptyState, StatusBadge, StatusDot } from "@/components/console/status";
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
  const [openLog, setOpenLog] = useState<string | null>(null);

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
          `${tenant.domain} ${tenant.title} ${tenant.runtime_name}`.toLowerCase().includes(needle)),
    );
  }, [tenants, filter, query]);

  const tenantsPerRuntime = useMemo(() => {
    const map = new Map<string, number>();
    for (const tenant of tenants) {
      map.set(tenant.runtime_name, (map.get(tenant.runtime_name) || 0) + 1);
    }
    return map;
  }, [tenants]);

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
              index === 1 && "md:border-r",
              index === 2 && "md:border-r",
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
        <CardHeader className="p-5">
          <CardTitle>Runtime releases</CardTitle>
          <CardDescription>
            Eunha ships once per shared runtime. Only one runtime deploys at a time.
          </CardDescription>
        </CardHeader>
        <Separator />
        {runtimes.length === 0 ? (
          <EmptyState icon={<Server />}>No runtimes are registered.</EmptyState>
        ) : (
          runtimes.map((runtime, index) => {
            const status = runtime.release_status || "idle";
            const lastLine = runtime.release_message?.trimEnd().split("\n").pop();
            const logOpen = openLog === runtime.id;
            const tenantCount = tenantsPerRuntime.get(runtime.name) || 0;
            return (
              <div key={runtime.id} className={cn(index > 0 && "border-border border-t")}>
                <div className="grid items-center gap-4 p-5 lg:grid-cols-[minmax(11rem,1fr)_minmax(14rem,1.4fr)_auto_auto]">
                  <div className="min-w-0">
                    <h3 className="font-semibold">{runtime.name}</h3>
                    <p className="text-muted-foreground mt-0.5 text-sm">
                      {runtime.bind_address} · {tenantCount} tenant{tenantCount === 1 ? "" : "s"}
                    </p>
                    <p
                      className="text-muted-foreground mt-0.5 font-mono text-xs"
                      title={runtime.deployed_ref || undefined}
                    >
                      {shortRef(runtime.deployed_ref)}
                    </p>
                  </div>
                  <div className="grid min-w-0 justify-items-start gap-1.5">
                    <StatusBadge status={status} />
                    <p className="text-muted-foreground text-sm break-words">
                      {lastLine || "No release activity recorded."}
                    </p>
                    {runtime.release_operation_id && (
                      <Button
                        variant="link"
                        size="sm"
                        className="text-primary h-auto p-0"
                        aria-expanded={logOpen}
                        onClick={() => setOpenLog(logOpen ? null : runtime.id)}
                      >
                        {logOpen ? <ChevronDown /> : <ChevronRight />}
                        {logOpen ? "Hide release log" : "View release log"}
                      </Button>
                    )}
                  </div>
                  <div className="flex items-center gap-2">
                    <Switch
                      id={`auto-${runtime.id}`}
                      checked={runtime.auto_deploy_enabled}
                      disabled={busy === `auto-${runtime.id}`}
                      onCheckedChange={(enabled) =>
                        act(`/runtimes/${runtime.id}/automatic-release`, `auto-${runtime.id}`, { enabled })
                      }
                    />
                    <Label htmlFor={`auto-${runtime.id}`} className="text-muted-foreground">
                      Auto deploy
                    </Label>
                  </div>
                  <ActionButton
                    variant="default"
                    icon={<Rocket />}
                    busy={busy === `release-${runtime.id}`}
                    disabled={status === "running"}
                    disabledReason="A release for this runtime is already in progress."
                    confirm={{
                      title: `Deploy the latest Eunha to ${runtime.name}?`,
                      description: `This builds the current central release and rolls it out to every tenant on ${runtime.name} (${tenantCount} tenant${tenantCount === 1 ? "" : "s"}). The runtime restarts, and a failed health check restores the previous release.`,
                      action: "Deploy to the fleet",
                    }}
                    onClick={() => act(`/runtimes/${runtime.id}/release`, `release-${runtime.id}`)}
                  >
                    Deploy latest
                  </ActionButton>
                </div>
                {logOpen && runtime.release_operation_id && (
                  <div className="bg-muted/40 border-border border-t">
                    <OperationLog
                      operationId={runtime.release_operation_id}
                      live={!isSettled(runtime.release_status)}
                    />
                  </div>
                )}
              </div>
            );
          })
        )}
      </Card>

      <Card className="gap-0 overflow-hidden p-0">
        <CardHeader className="flex flex-col gap-4 p-5 lg:flex-row lg:items-start lg:justify-between">
          <div>
            <CardTitle>Tenant inventory</CardTitle>
            <CardDescription>
              Showing {visible.length} of {tenants.length} tenants.
            </CardDescription>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <SearchField
              value={query}
              onChange={setQuery}
              label="Search tenants"
              placeholder="Search domain, title, or runtime"
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
          visible.map((tenant, index) => (
            <div
              key={tenant.id}
              className={cn(
                "hover:bg-muted/40 grid items-center gap-4 p-5 transition-colors",
                "grid-cols-[auto_1fr] xl:grid-cols-[auto_minmax(12rem,1fr)_minmax(20rem,1.3fr)_auto]",
                index > 0 && "border-border border-t",
              )}
            >
              <StatusDot state={tenant.observed_state} />
              <div className="min-w-0">
                <h3 className="font-semibold">{tenant.domain}</h3>
                <p className="text-muted-foreground mt-0.5 text-sm">{tenant.title}</p>
                <div className="mt-1.5 flex flex-wrap gap-1.5 empty:hidden">
                  {pendingRender(tenant) && (
                    <Badge variant="outline" className="border-pending/40 bg-pending/10 text-pending">
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
                <div>
                  <dt className="text-muted-foreground text-xs tracking-[0.08em] uppercase">Runtime</dt>
                  <dd className="mt-0.5 text-sm break-words">
                    {tenant.runtime_name}
                    <span className="text-muted-foreground"> · {tenant.bind_address}</span>
                  </dd>
                </div>
                <div>
                  <dt className="text-muted-foreground text-xs tracking-[0.08em] uppercase">Config</dt>
                  <dd className="mt-0.5 text-sm break-words">
                    revision {tenant.config_revision}
                    {pendingRender(tenant) && (
                      <span className="text-pending"> · rendered {tenant.rendered_revision ?? "—"}</span>
                    )}
                  </dd>
                </div>
                <div>
                  <dt className="text-muted-foreground text-xs tracking-[0.08em] uppercase">State</dt>
                  <dd className="mt-0.5 text-sm break-words">
                    {tenant.observed_state}
                    <span className="text-muted-foreground" title={absoluteTime(tenant.updated_at)}>
                      {" "}
                      · {relativeTime(tenant.updated_at)}
                    </span>
                  </dd>
                </div>
              </dl>
              <div className="col-span-2 flex flex-wrap gap-2 xl:col-span-1 xl:justify-end">
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
          ))
        )}
      </Card>
    </div>
  );
}
