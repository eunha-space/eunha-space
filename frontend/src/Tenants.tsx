import { useMemo, useState, type ReactNode } from "react";
import { Download, ExternalLink, FileCog, Globe, HardDrive, HeartPulse, Inbox, Plus, PowerOff, RefreshCw, RotateCcw, RotateCw, Trash2 } from "lucide-react";
import { api, type DomainChange, instanceAddress, isOffline, pendingRender, type Runtime, type Tenant, type TenantDomain } from "@/api";
import { absoluteTime, relativeTime } from "@/format";
import { Badge } from "@/components/ui/badge";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { Separator } from "@/components/ui/separator";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
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

/**
 * One column track shared by the header and every row, so the inventory reads
 * as a table: the labels are printed once at the top instead of beside each
 * value. Below `xl` the metadata folds into a wrapping strip and carries its
 * own inline labels again.
 */
// Every track is content-independent: each row is its own grid, so a track
// that sized to its content would leave a row with fewer actions or a longer
// domain out of step with the rest of the column.
const COLUMNS = cn(
  "grid items-center gap-x-3 gap-y-1 grid-cols-[0.625rem_1fr]",
  "xl:grid-cols-[0.625rem_minmax(0,1.6fr)_minmax(0,1.1fr)_minmax(0,0.6fr)_minmax(0,0.9fr)_minmax(0,1.1fr)_12rem]",
);

const HEADINGS = ["Instance", "Runtime", "Config", "State", "Lifecycle"];

function Cell({ label, children, title }: { label: string; children: ReactNode; title?: string }) {
  return (
    <div className="min-w-0 truncate text-xs" title={title}>
      <span className="text-muted-foreground mr-1 text-[10px] tracking-[0.08em] uppercase xl:hidden">
        {label}
      </span>
      {children}
    </div>
  );
}

/** Row badges stay on the name line, so they are sized to sit in it. */
function RowBadge({ className, title, children }: { className?: string; title?: string; children: ReactNode }) {
  return (
    <Badge variant="outline" className={cn("px-1.5 py-0 text-[10px] font-normal", className)} title={title}>
      {children}
    </Badge>
  );
}


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
              "hover:bg-muted/60 focus-visible:ring-ring/50 cursor-pointer px-4 py-2.5 text-left transition-colors focus-visible:ring-3 focus-visible:outline-none",
              "border-border border-b md:border-b-0",
              index % 2 === 0 && "border-r",
              (index === 1 || index === 2) && "md:border-r",
              filter === key && "bg-accent/60 shadow-[inset_0_2px_0_var(--primary)]",
            )}
          >
            <span className="text-muted-foreground block text-xs">{label}</span>
            <strong
              className={cn(
                "block text-xl font-semibold tabular-nums",
                key === "attention" && counts.attention > 0 && "text-destructive",
              )}
            >
              {counts[key]}
            </strong>
          </button>
        ))}
      </Card>

      <Card className="gap-0 overflow-hidden p-0">
        <CardHeader className="flex flex-col gap-3 px-4 py-3 xl:flex-row xl:items-center xl:justify-between">
          <div>
            <CardTitle className="text-base">Tenant inventory</CardTitle>
            <CardDescription className="text-xs">
              Showing {visible.length} of {tenants.length} tenants.
            </CardDescription>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <CreateTenantDialog runtimes={runtimes} busy={busy === "create-tenant"} act={act} />
            <ImportTenantDialog runtimes={runtimes} busy={busy === "import-tenant"} act={act} />
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
        {visible.length > 0 && (
          <div
            className={cn(
              COLUMNS,
              "text-muted-foreground border-border hidden border-b px-4 py-1.5 text-[10px] tracking-[0.08em] uppercase xl:grid",
            )}
            aria-hidden
          >
            <span />
            {HEADINGS.map((heading) => <span key={heading}>{heading}</span>)}
            <span className="justify-self-end">Actions</span>
          </div>
        )}
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
                <div className="bg-muted/50 border-border flex items-center gap-2 border-y px-4 py-1.5 first:border-t-0">
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
                    COLUMNS,
                    "hover:bg-muted/40 px-4 py-2 transition-colors",
                    index > 0 && "border-border border-t",
                  )}
                >
                  <StatusDot state={tenant.observed_state} />
                  <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
                    <h3 className="truncate text-sm font-semibold" title={instanceAddress(tenant)}>
                      {instanceAddress(tenant)}
                    </h3>
                    <span className="text-muted-foreground truncate text-xs" title={tenant.title}>
                      {tenant.title}
                    </span>
                    {tenant.imported && (
                      <RowBadge className="text-muted-foreground" title="Taken over from an existing Mastodon instance; its database and media predate the platform.">
                        Imported
                      </RowBadge>
                    )}
                    {pendingRender(tenant) && (
                      <RowBadge className="border-pending/40 bg-pending/10 text-pending">
                        Pending render
                      </RowBadge>
                    )}
                    {tenant.lifecycle_state !== "ready" && (
                      <RowBadge className={tenant.lifecycle_state === "failed" ? "text-destructive" : "text-pending"}>
                        {tenant.lifecycle_state}
                      </RowBadge>
                    )}
                    {tenant.purge_scheduled_for && (
                      <RowBadge className="text-destructive" title={absoluteTime(tenant.purge_scheduled_for)}>
                        purges {relativeTime(tenant.purge_scheduled_for)}
                      </RowBadge>
                    )}
                    {/* A deleted tenant's lifecycle badge already says it is going or has gone offline. */}
                    {tenant.desired_state !== "running" && !isOffline(tenant) && (
                      <RowBadge className="text-muted-foreground">
                        Desired: {tenant.desired_state}
                      </RowBadge>
                    )}
                  </div>
                  {/* Wide screens align these against the column header; narrow
                      ones fold them into one wrapping strip under the name. */}
                  <div className="max-xl:col-span-2 max-xl:flex max-xl:flex-wrap max-xl:gap-x-4 max-xl:gap-y-0.5 xl:contents">
                    <Cell label="Runtime">
                      {tenant.runtime_name}
                      <span className="text-muted-foreground"> · {tenant.bind_address}</span>
                    </Cell>
                    <Cell label="Config">
                      rev {tenant.config_revision}
                      {pendingRender(tenant) && (
                        <span className="text-pending"> · rendered {tenant.rendered_revision ?? "—"}</span>
                      )}
                    </Cell>
                    <Cell label="State">
                      {tenant.observed_state}
                      <span className="text-muted-foreground" title={absoluteTime(tenant.updated_at)}>
                        {" "}· {relativeTime(tenant.updated_at)}
                      </span>
                    </Cell>
                    <Cell label="Lifecycle" title={tenant.temporal_workflow_id ?? undefined}>
                      {tenant.temporal_workflow_id
                        ? (
                          <>
                            <LifecycleExecution tenant={tenant} />
                            {/* A plain `&&` on the count would print a literal 0. */}
                            {(tenant.temporal_dispatch_attempts ?? 0) > 1 && (
                              <span className="text-muted-foreground">
                                {" "}· dispatch {tenant.temporal_dispatch_attempts}
                              </span>
                            )}
                          </>
                        )
                        : <span className="text-muted-foreground">—</span>}
                    </Cell>
                  </div>
                  <div className="flex flex-wrap items-center gap-1 max-xl:col-span-2 xl:flex-nowrap xl:justify-end">
                    {tenant.lifecycle_state === "failed" && (
                      <ActionButton
                        iconOnly
                        icon={<RotateCw />}
                        busy={busy === `retry-${tenant.id}`}
                        onClick={() => act(`/tenants/${tenant.id}/retry`, `retry-${tenant.id}`)}
                      >
                        Retry provisioning
                      </ActionButton>
                    )}
                    {tenant.lifecycle_state === "retained" && (
                      <ActionButton
                        iconOnly
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
                    )}
                    {["provisioning", "ready", "failed"].includes(tenant.lifecycle_state) && (
                      <ActionButton
                        iconOnly
                        icon={<PowerOff />}
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
                          iconOnly
                          icon={<HeartPulse />}
                          busy={busy === `check-${tenant.id}`}
                          onClick={() => act(`/tenants/${tenant.id}/check`, `check-${tenant.id}`)}
                        >
                          Check
                        </ActionButton>
                        <ActionButton
                          iconOnly
                          variant="default"
                          icon={<RefreshCw />}
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
      <Tooltip>
        <TooltipTrigger render={<span className="inline-flex" />}>
          <DialogTrigger render={<Button variant="destructive" size="icon-sm" aria-label="Purge" />}>
            <Trash2 />
          </DialogTrigger>
        </TooltipTrigger>
        <TooltipContent>Purge</TooltipContent>
      </Tooltip>
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

/**
 * Take over an existing Mastodon instance.
 *
 * Almost every field here exists because the instance already has data that
 * depends on it. The encryption secrets are what its accounts' signing keys are
 * encrypted with, the VAPID keypair is the one its subscribers already hold,
 * and its media has to keep the addresses it has already handed out. Generating
 * any of these afresh would produce a server that cannot sign, cannot push, and
 * shows no pictures — so they are asked for rather than made.
 */
function ImportTenantDialog({ runtimes, busy, act }: { runtimes: Runtime[]; busy: boolean; act: Props["act"] }) {
  const [open, setOpen] = useState(false);
  const [domain, setDomain] = useState("");
  const [title, setTitle] = useState("");
  const [email, setEmail] = useState("");
  const [runtimeId, setRuntimeId] = useState("");
  const [dumpPath, setDumpPath] = useState("");
  const [mediaPath, setMediaPath] = useState("");
  const [ownerUsername, setOwnerUsername] = useState("");
  const [encryptionKey, setEncryptionKey] = useState("");
  const [encryptionSalt, setEncryptionSalt] = useState("");
  const [vapidPrivate, setVapidPrivate] = useState("");
  const [vapidPublic, setVapidPublic] = useState("");
  const selectedRuntime = runtimeId || runtimes[0]?.id || "";

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    const ok = await act("/tenants/import", "import-tenant", {
      domain, title, contact_email: email || null, runtime_id: selectedRuntime,
      dump_path: dumpPath, media_path: mediaPath || null,
      owner_username: ownerUsername || null,
      active_record_encryption_primary_key: encryptionKey,
      active_record_encryption_key_derivation_salt: encryptionSalt,
      vapid_private_key: vapidPrivate || null, vapid_public_key: vapidPublic || null,
    });
    if (ok) {
      setOpen(false);
      setDomain(""); setTitle(""); setEmail(""); setDumpPath(""); setMediaPath("");
      setOwnerUsername(""); setEncryptionKey(""); setEncryptionSalt("");
      setVapidPrivate(""); setVapidPublic("");
    }
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger render={<Button variant="outline" />}><Download />Import instance</DialogTrigger>
      <DialogContent className="max-h-[85vh] overflow-y-auto">
        <form onSubmit={submit} className="grid gap-5">
          <DialogHeader>
            <DialogTitle>Import a Mastodon instance</DialogTitle>
            <DialogDescription>The instance is restored, served and health-checked before its domain moves, because moving its domain is the cutover. It goes live when its DNS points here and Cloudflare validates the hostname, not before.</DialogDescription>
          </DialogHeader>
          <div className="grid gap-4">
            <div className="grid gap-2">
              <Label htmlFor="import-domain">Instance domain</Label>
              <Input id="import-domain" value={domain} onChange={(e) => setDomain(e.target.value.toLowerCase())} placeholder="seoul.earth" required />
              <p className="text-muted-foreground text-xs">It keeps this domain. Its accounts&rsquo; identity is on it and remote servers know them there, so an import is never also a move.</p>
            </div>
            <div className="grid gap-2"><Label htmlFor="import-title">Name</Label><Input id="import-title" value={title} onChange={(e) => setTitle(e.target.value)} required /></div>
            <div className="grid gap-2"><Label htmlFor="import-email">Contact email</Label><Input id="import-email" type="email" value={email} onChange={(e) => setEmail(e.target.value)} /></div>
            <div className="grid gap-2"><Label htmlFor="import-runtime">Runtime</Label><select id="import-runtime" className="border-input bg-background h-9 rounded-md border px-3 text-sm" value={selectedRuntime} onChange={(e) => setRuntimeId(e.target.value)} required>{runtimes.map((runtime) => <option key={runtime.id} value={runtime.id}>{runtime.instance_name} / {runtime.name}</option>)}</select></div>
            <Separator />
            <div className="grid gap-2">
              <Label htmlFor="import-dump">Database dump</Label>
              <Input id="import-dump" value={dumpPath} onChange={(e) => setDumpPath(e.target.value)} placeholder="/srv/imports/seoul.earth.custom" pattern="/[A-Za-z0-9._/-]+" required />
              <p className="text-muted-foreground text-xs">An absolute path on the runtime host to a <code>pg_dump -Fc</code> of the instance, taken from the Mastodon release this fleet builds.</p>
            </div>
            <div className="grid gap-2">
              <Label htmlFor="import-media">Media directory</Label>
              <Input id="import-media" value={mediaPath} onChange={(e) => setMediaPath(e.target.value)} placeholder="/srv/imports/seoul.earth/system" pattern="/[A-Za-z0-9._/-]+" />
              <p className="text-muted-foreground text-xs">The instance&rsquo;s <code>public/system</code> tree, staged beside the dump. Its own files are uploaded; its cache of other servers&rsquo; media is left behind and fetched again on demand. Leave empty for an instance whose storage goes on serving its media where it is.</p>
            </div>
            <div className="grid gap-2">
              <Label htmlFor="import-owner">Admin username</Label>
              <Input id="import-owner" value={ownerUsername} onChange={(e) => setOwnerUsername(e.target.value)} placeholder="admin" pattern="[A-Za-z0-9_]{1,30}" />
              <p className="text-muted-foreground text-xs">An account that already exists in the dump. Recorded, not created.</p>
            </div>
            <Separator />
            <div className="grid gap-2">
              <Label htmlFor="import-encryption-key">ActiveRecord encryption key</Label>
              <Input id="import-encryption-key" value={encryptionKey} onChange={(e) => setEncryptionKey(e.target.value)} autoComplete="off" required />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="import-encryption-salt">ActiveRecord key derivation salt</Label>
              <Input id="import-encryption-salt" value={encryptionSalt} onChange={(e) => setEncryptionSalt(e.target.value)} autoComplete="off" required />
              <p className="text-muted-foreground text-xs">The source instance&rsquo;s own. Its accounts&rsquo; signing keys are encrypted with these, and without them it cannot sign anything it federates.</p>
            </div>
            <div className="grid gap-2">
              <Label htmlFor="import-vapid-private">VAPID private key</Label>
              <Input id="import-vapid-private" value={vapidPrivate} onChange={(e) => setVapidPrivate(e.target.value)} autoComplete="off" />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="import-vapid-public">VAPID public key</Label>
              <Input id="import-vapid-public" value={vapidPublic} onChange={(e) => setVapidPublic(e.target.value)} autoComplete="off" />
              <p className="text-muted-foreground text-xs">Both or neither. Leaving them empty generates a new pair, which silently ends every Web Push subscription the instance has handed out.</p>
            </div>
          </div>
          <DialogFooter><Button type="submit" disabled={busy || !selectedRuntime}>{busy ? "Importing…" : "Import instance"}</Button></DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function DomainsDialog({ tenant, busy, act }: { tenant: Tenant; busy: string; act: Props["act"] }) {
  const [open, setOpen] = useState(false);
  const [domains, setDomains] = useState<TenantDomain[]>([]);
  const [change, setChange] = useState<DomainChange | null>(null);
  const [target, setTarget] = useState("");
  async function refresh() {
    const [nextDomains, nextChange] = await Promise.all([
      api<TenantDomain[]>(`/tenants/${tenant.id}/domains`),
      api<DomainChange | null>(`/tenants/${tenant.id}/domain-change`),
    ]);
    setDomains(nextDomains);
    setChange(nextChange);
  }
  async function changeAct(path: string, key: string, body?: object) {
    if (await act(`/tenants/${tenant.id}/domain-change${path}`, key, body)) {
      setTarget("");
      await refresh();
    }
  }
  // A bare label is an eunha space subdomain; anything with a dot, a domain.
  const request = target.includes(".") ? { custom_domain: target } : { subdomain: target };
  async function adopt(domain: TenantDomain) {
    if (await act(`/tenants/${tenant.id}/domains/${domain.id}/cloudflare`, `adopt-domain-${domain.id}`)) {
      await refresh();
    }
  }
  return <Dialog open={open} onOpenChange={(next) => { setOpen(next); if (next) void refresh(); }}>
    <Tooltip>
      <TooltipTrigger render={<span className="inline-flex" />}>
        <DialogTrigger render={<Button variant="outline" size="icon-sm" aria-label="Domains" />}>
          <Globe />
        </DialogTrigger>
      </TooltipTrigger>
      <TooltipContent>Domains</TooltipContent>
    </Tooltip>
    <DialogContent>
      <DialogHeader><DialogTitle>Domains for {tenant.domain}</DialogTitle><DialogDescription>Tenant routing and Cloudflare certificates. Adopting an existing primary domain does not change its ActivityPub identity; changing the domain moves it, with a Move to every account's followers.</DialogDescription></DialogHeader>
      <div className="bg-muted/50 grid gap-2 rounded-md p-3 text-sm">
        <div className="flex items-center justify-between gap-2">
          <span className="font-medium">Domain change</span>
          {change && <Badge variant={change.status === "succeeded" ? "secondary" : "outline"}>{change.status}{change.status === "running" ? ` · ${change.step}` : ""}</Badge>}
        </div>
        {change && <p className="text-muted-foreground text-xs">{change.from_domain} → {change.to_domain} · started {absoluteTime(change.created_at)}{change.pending !== null ? ` · ${change.pending} deliveries pending, ${change.given_up ?? 0} given up on` : ""}</p>}
        {change?.error && <p className="text-destructive text-xs">{change.error}</p>}
        <div className="flex flex-wrap gap-2">
          {change?.status === "running" && change.step === "attach" && <ActionButton variant="outline" busy={busy === `cancel-change-${tenant.id}`} onClick={() => changeAct("/cancel", `cancel-change-${tenant.id}`)}>Cancel change</ActionButton>}
          {change?.status === "failed" && change.step !== "attach" && <ActionButton variant="outline" busy={busy === `retry-change-${tenant.id}`} onClick={() => changeAct("/retry", `retry-change-${tenant.id}`)}>Retry from {change.step}</ActionButton>}
        </div>
        {change?.status !== "running" && tenant.lifecycle_state === "ready" && <div className="flex gap-2">
          <Input value={target} onChange={(e) => setTarget(e.target.value.trim().toLowerCase())} placeholder="label or custom.domain" aria-label="New domain" />
          <ActionButton
            variant="outline"
            disabled={!target}
            busy={busy === `start-change-${tenant.id}`}
            confirm={{
              title: `Move ${tenant.domain} to ${target.includes(".") ? target : `${target}.eunha.space`}?`,
              description: "Its addresses are rewritten, it is served under the new domain alone, and every account's followers are sent a Move. The old domain stops answering once the move begins; this cannot be undone after the new domain connects.",
              action: "Start move",
            }}
            onClick={() => changeAct("", `start-change-${tenant.id}`, request)}
          >Change</ActionButton>
        </div>}
      </div>
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
