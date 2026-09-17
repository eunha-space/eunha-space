import { useMemo, useState } from "react";
import { ExternalLink, HardDrive, Inbox, Plus, RefreshCw, RotateCcw, Trash2 } from "lucide-react";
import { api, instanceAddress, isOffline, pendingRender, type Runtime, type Tenant, type TenantDomain } from "@/api";
import { absoluteTime, relativeTime } from "@/format";
import { Badge } from "@/components/ui/badge";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
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
  act: (path: string, key: string, body?: object) => Promise<boolean>;
};

const NEEDS_ATTENTION = new Set(["unhealthy", "unknown"]);


function matches(tenant: Tenant, filter: string): boolean {
  if (filter === "healthy") return tenant.observed_state === "healthy";
  if (filter === "attention") return NEEDS_ATTENTION.has(tenant.observed_state);
  if (filter === "pending") return pendingRender(tenant);
  return true;
}

export function Tenants({ tenants, runtimes, busy, act }: Props) {
  const [filter, setFilter] = useState("all");
  const [query, setQuery] = useState("");

  const hostByRuntime = useMemo(
    () => new Map(runtimes.map((runtime) => [runtime.name, runtime.instance_name])),
    [runtimes],
  );
  const hostCount = useMemo(() => new Set(runtimes.map((r) => r.instance_name)).size, [runtimes]);

  // Grouping only earns its headers once the fleet spans more than one instance.
  const [grouped, setGrouped] = useState<boolean | null>(null);
  const groupByInstance = grouped ?? hostCount > 1;

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
          `${instanceAddress(tenant)} ${tenant.title} ${tenant.runtime_name} ${hostByRuntime.get(tenant.runtime_name) ?? ""}`
            .toLowerCase()
            .includes(needle)),
    );
  }, [tenants, filter, query, hostByRuntime]);

  const groups = useMemo(() => {
    if (!groupByInstance) return [["", visible]] as [string, Tenant[]][];
    const byHost = new Map<string, Tenant[]>();
    for (const tenant of visible) {
      const host = hostByRuntime.get(tenant.runtime_name) ?? "unknown instance";
      const existing = byHost.get(host);
      if (existing) existing.push(tenant);
      else byHost.set(host, [tenant]);
    }
    return [...byHost.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [visible, groupByInstance, hostByRuntime]);

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
            <CreateTenantDialog runtimes={runtimes} busy={busy === "create-tenant"} act={act} />
            <div className="flex items-center gap-2">
              <Switch
                id="group-by-instance"
                checked={groupByInstance}
                onCheckedChange={(next) => setGrouped(next)}
              />
              <Label htmlFor="group-by-instance" className="text-muted-foreground whitespace-nowrap">
                Group by instance
              </Label>
            </div>
            <SearchField
              value={query}
              onChange={setQuery}
              label="Search tenants"
              placeholder="Search domain, runtime, or instance"
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
              {groupByInstance && (
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
                    "grid-cols-[auto_1fr] xl:grid-cols-[auto_minmax(10rem,16rem)_minmax(0,1fr)_auto]",
                    index > 0 && "border-border border-t",
                  )}
                >
                  <StatusDot state={tenant.observed_state} />
                  <div className="min-w-0">
                    <h3 className="truncate font-semibold" title={instanceAddress(tenant)}>
                      {instanceAddress(tenant)}
                    </h3>
                    <p className="text-muted-foreground mt-0.5 truncate text-sm" title={tenant.title}>
                      {tenant.title}
                    </p>
                    <div className="mt-1.5 flex flex-wrap gap-1.5 empty:hidden">
                      {pendingRender(tenant) && (
                        <Badge
                          variant="outline"
                          className="border-pending/40 bg-pending/10 text-pending"
                        >
                          Pending render
                        </Badge>
                      )}
                      {tenant.lifecycle_state !== "ready" && (
                        <Badge variant="outline" className={tenant.lifecycle_state === "failed" ? "text-destructive" : "text-pending"}>
                          {tenant.lifecycle_state}
                        </Badge>
                      )}
                      {tenant.purge_scheduled_for && (
                        <Badge variant="outline" className="text-destructive" title={absoluteTime(tenant.purge_scheduled_for)}>
                          purges {relativeTime(tenant.purge_scheduled_for)}
                        </Badge>
                      )}
                      {/* A deleted tenant's lifecycle badge already says it is going or has gone offline. */}
                      {tenant.desired_state !== "running" &&
                        !isOffline(tenant) && (
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
                    {tenant.temporal_workflow_id && (
                      <div className="min-w-0">
                        <dt className="text-muted-foreground text-xs tracking-[0.08em] uppercase">
                          Lifecycle
                        </dt>
                        <dd
                          className="mt-0.5 truncate font-mono text-sm"
                          title={tenant.temporal_workflow_id}
                        >
                          <LifecycleExecution tenant={tenant} />
                          {tenant.temporal_dispatch_attempts && tenant.temporal_dispatch_attempts > 1 && (
                            <span className="text-muted-foreground">
                              {" "}· dispatch {tenant.temporal_dispatch_attempts}
                            </span>
                          )}
                        </dd>
                      </div>
                    )}
                  </dl>
                  <div className="col-span-2 flex flex-wrap gap-2 xl:col-span-1 xl:justify-end">
                    {tenant.lifecycle_state === "failed" && (
                      <ActionButton busy={busy === `retry-${tenant.id}`} onClick={() => act(`/tenants/${tenant.id}/retry`, `retry-${tenant.id}`)}>
                        Retry provisioning
                      </ActionButton>
                    )}
                    {tenant.lifecycle_state === "retained" && (
                      <>
                        <ActionButton
                          icon={<RotateCcw />}
                          busy={busy === `restore-${tenant.id}`}
                          confirm={{
                            title: `Restore ${instanceAddress(tenant)}?`,
                            description: "The tenant is migrated to its runtime's release, rendered and served again with its retained data. The customer's instance limit is not checked.",
                            action: "Restore",
                          }}
                          onClick={() => act(`/tenants/${tenant.id}/restore`, `restore-${tenant.id}`)}
                        >
                          Restore
                        </ActionButton>
                      </>
                    )}
                    {["provisioning", "ready", "failed"].includes(tenant.lifecycle_state) && (
                      <ActionButton
                        icon={<Trash2 />}
                        variant="outline"
                        busy={busy === `deprovision-${tenant.id}`}
                        confirm={{
                          title: `Take ${instanceAddress(tenant)} offline?`,
                          description: "The tenant is taken offline now. Its database, media, configuration and backups are retained for 7 days and can be restored during that period.",
                          action: "Take offline",
                        }}
                        onClick={() => act(`/tenants/${tenant.id}/deprovision`, `deprovision-${tenant.id}`)}
                      >
                        Take offline
                      </ActionButton>
                    )}
                    {tenant.lifecycle_state !== "purging" && tenant.lifecycle_state !== "purged" && (
                      <>
                        <DomainsDialog tenant={tenant} busy={busy} act={act} />
                        <TenantConfigDialog tenantId={tenant.id} domain={tenant.domain} />
                      </>
                    )}
                    {(!isOffline(tenant) || tenant.lifecycle_state === "retained") && (
                      <PurgeDialog tenant={tenant} busy={busy === `purge-${tenant.id}`} act={act} />
                    )}
                    {!isOffline(tenant) && (
                      <>
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
                      </>
                    )}
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

/** The tenant's Temporal execution: what it is doing, and where to watch it.
 *  The link only opens the execution in the operator's Temporal UI; workflow
 *  inputs, results and history stay out of this console. */
function LifecycleExecution({ tenant }: { tenant: Tenant }) {
  const label = tenant.lifecycle_step
    ? `${tenant.temporal_workflow_kind} · ${tenant.lifecycle_step}`
    : tenant.temporal_workflow_kind;
  // No configured Temporal UI, no link: the service is never public, so there
  // is no address to guess.
  if (!tenant.temporal_workflow_url) return <>{label}</>;
  return (
    <a
      className="inline-flex max-w-full items-center gap-1 underline-offset-4 hover:underline"
      href={tenant.temporal_workflow_url}
      target="_blank"
      rel="noreferrer"
      aria-label={`Open ${label} in the Temporal UI`}
    >
      <span className="truncate">{label}</span>
      <ExternalLink className="size-3 shrink-0" aria-hidden />
    </a>
  );
}

function PurgeDialog({ tenant, busy, act }: { tenant: Tenant; busy: boolean; act: Props["act"] }) {
  const [open, setOpen] = useState(false);
  const [confirmation, setConfirmation] = useState("");
  const address = instanceAddress(tenant);
  const confirmed = confirmation.trim().toLowerCase() === address.toLowerCase();
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setConfirmation("");
      }}
    >
      <DialogTrigger render={<Button variant="destructive" />}>
        <Trash2 /> Purge
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Purge {address}?</DialogTitle>
          <DialogDescription>
            {tenant.lifecycle_state === "retained"
              ? "This permanently destroys the tenant: "
              : "This takes the tenant offline immediately, with no grace period for its customer, and permanently destroys it: "}
            its Cloudflare hostnames, database and role, media, Redis keys and configuration.
            It cannot be restored afterwards. The domain stays reserved so it is never reused.
          </DialogDescription>
        </DialogHeader>
        <form
          className="grid gap-2"
          onSubmit={async (event) => {
            event.preventDefault();
            if (!confirmed) return;
            if (await act(`/tenants/${tenant.id}/purge`, `purge-${tenant.id}`, { confirm_domain: confirmation })) {
              setOpen(false);
              setConfirmation("");
            }
          }}
        >
          <Label htmlFor={`purge-${tenant.id}`}>Type {address} to confirm</Label>
          <Input
            id={`purge-${tenant.id}`}
            autoComplete="off"
            value={confirmation}
            onChange={(event) => setConfirmation(event.target.value)}
          />
          <DialogFooter>
            <Button type="submit" variant="destructive" disabled={!confirmed || busy}>
              Purge permanently
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function CreateTenantDialog({ runtimes, busy, act }: { runtimes: Runtime[]; busy: boolean; act: Props["act"] }) {
  const [open, setOpen] = useState(false);
  const [subdomain, setSubdomain] = useState("");
  const [title, setTitle] = useState("");
  const [email, setEmail] = useState("");
  const [runtimeId, setRuntimeId] = useState("");
  const [registrationsOpen, setRegistrationsOpen] = useState(true);
  const [approvalRequired, setApprovalRequired] = useState(false);
  const selectedRuntime = runtimeId || runtimes[0]?.id || "";

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    const ok = await act("/tenants", "create-tenant", {
      subdomain, title, contact_email: email || null, runtime_id: selectedRuntime,
      registrations_open: registrationsOpen, approval_required: approvalRequired,
    });
    if (ok) { setOpen(false); setSubdomain(""); setTitle(""); setEmail(""); }
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger render={<Button />}><Plus />Provision tenant</DialogTrigger>
      <DialogContent>
        <form onSubmit={submit} className="grid gap-5">
          <DialogHeader>
            <DialogTitle>Provision a tenant</DialogTitle>
            <DialogDescription>The control plane creates isolated PostgreSQL credentials and UUID-scoped Redis and media keys, then migrates and activates the tenant.</DialogDescription>
          </DialogHeader>
          <div className="grid gap-4">
            <div className="grid gap-2"><Label htmlFor="tenant-subdomain">Eunha hostname</Label><div className="flex items-center"><Input id="tenant-subdomain" className="rounded-r-none" value={subdomain} onChange={(e) => setSubdomain(e.target.value.toLowerCase())} placeholder="example" pattern="[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?" required /><span className="border-input bg-muted text-muted-foreground flex h-9 items-center rounded-r-md border border-l-0 px-3 text-sm">.eunha.space</span></div></div>
            <div className="grid gap-2"><Label htmlFor="tenant-title">Name</Label><Input id="tenant-title" value={title} onChange={(e) => setTitle(e.target.value)} required /></div>
            <div className="grid gap-2"><Label htmlFor="tenant-email">Contact email</Label><Input id="tenant-email" type="email" value={email} onChange={(e) => setEmail(e.target.value)} /></div>
            <div className="grid gap-2"><Label htmlFor="tenant-runtime">Runtime</Label><select id="tenant-runtime" className="border-input bg-background h-9 rounded-md border px-3 text-sm" value={selectedRuntime} onChange={(e) => setRuntimeId(e.target.value)} required>{runtimes.map((runtime) => <option key={runtime.id} value={runtime.id}>{runtime.instance_name} / {runtime.name}</option>)}</select></div>
            <div className="flex items-center justify-between gap-4"><Label htmlFor="registrations-open">Open registrations</Label><Switch id="registrations-open" checked={registrationsOpen} onCheckedChange={setRegistrationsOpen} /></div>
            <div className="flex items-center justify-between gap-4"><Label htmlFor="approval-required">Require approval</Label><Switch id="approval-required" checked={approvalRequired} onCheckedChange={setApprovalRequired} /></div>
          </div>
          <DialogFooter><Button type="submit" disabled={busy || !selectedRuntime}>{busy ? "Provisioning…" : "Provision tenant"}</Button></DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function DomainsDialog({ tenant, busy, act }: { tenant: Tenant; busy: string; act: Props["act"] }) {
  const [open, setOpen] = useState(false);
  const [domains, setDomains] = useState<TenantDomain[]>([]);
  async function refresh() { setDomains(await api<TenantDomain[]>(`/tenants/${tenant.id}/domains`)); }
  async function adopt(domain: TenantDomain) {
    if (await act(`/tenants/${tenant.id}/domains/${domain.id}/cloudflare`, `adopt-domain-${domain.id}`)) {
      await refresh();
    }
  }
  return <Dialog open={open} onOpenChange={(next) => { setOpen(next); if (next) void refresh(); }}>
    <DialogTrigger render={<Button variant="outline" />}>Domains</DialogTrigger>
    <DialogContent>
      <DialogHeader><DialogTitle>Domains for {tenant.domain}</DialogTitle><DialogDescription>Tenant routing and Cloudflare certificates. Custom domains are chosen only when a customer creates an instance. Adopting an existing primary domain does not change its ActivityPub identity.</DialogDescription></DialogHeader>
      <div className="grid gap-3">{domains.map((domain) => <div key={domain.id} className="bg-muted/50 rounded-md p-3 text-sm">
        <div className="flex items-center justify-between gap-2"><div className="min-w-0"><span className="block truncate font-medium">{domain.hostname}</span><span className="text-muted-foreground text-xs">{domain.kind === "legacy" ? "Existing domain" : domain.kind === "provided" ? "Eunha hostname" : "Cloudflare for SaaS"}</span></div><div className="flex shrink-0 gap-1">{domain.is_primary && <Badge variant="outline">primary</Badge>}<Badge variant={domain.status === "active" ? "secondary" : "outline"}>{domain.status}</Badge>{domain.kind !== "legacy" && <Badge variant="secondary">managed</Badge>}</div></div>
        {domain.kind === "legacy" && domain.is_primary && <div className="mt-3 flex items-center justify-between gap-4 border-t pt-3"><p className="text-muted-foreground text-xs">Move TLS and routing to the shared Eunha tunnel without changing accounts or actor URLs.</p><ActionButton
          variant="outline"
          busy={busy === `adopt-domain-${domain.id}`}
          confirm={{
            title: `Manage ${domain.hostname} with Cloudflare?`,
            description: `Cloudflare for SaaS will issue and manage its certificate. The domain remains the tenant's primary ActivityPub identity. After starting, point its DNS to ${domain.cname_target}.`,
            action: "Start migration",
          }}
          onClick={() => adopt(domain)}
        >Manage with Cloudflare</ActionButton></div>}
        {domain.kind === "custom" && domain.status !== "active" && <div className="text-muted-foreground mt-2 grid gap-1 font-mono text-xs"><span>CNAME {domain.hostname} → {domain.cname_target}</span>{domain.ownership_name && domain.ownership_value && <span>TXT {domain.ownership_name} → {domain.ownership_value}</span>}<span>Cloudflare: {domain.cloudflare_status ?? "queued"} · TLS: {domain.ssl_status ?? "queued"}</span></div>}
        {domain.kind === "provided" && domain.status !== "active" && <div className="text-muted-foreground mt-2 font-mono text-xs">Cloudflare: {domain.cloudflare_status ?? "queued"} · TLS: {domain.ssl_status ?? "queued"}</div>}
        {domain.error && <p className="text-destructive mt-2">{domain.error}</p>}
      </div>)}</div>
    </DialogContent>
  </Dialog>;
}
