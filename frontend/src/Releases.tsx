import { useState } from "react";
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  ChevronRight,
  FastForward,
  Layers,
  Pause,
  Pencil,
  Play,
  Plus,
  Rocket,
  Server,
  Trash2,
} from "lucide-react";
import {
  isActiveRollout,
  isSettled,
  type ReleaseChannel,
  type Rollout,
  type Runtime,
} from "@/api";
import { absoluteTime, relativeTime, shortRef } from "@/format";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { ActionButton } from "@/components/console/action-button";
import { OperationLog } from "@/components/console/operation-log";
import { EmptyState, StatusBadge } from "@/components/console/status";
import { cn } from "cn";

export type Act = (path: string, key: string, body?: object, method?: string) => Promise<boolean>;

type Props = {
  runtimes: Runtime[];
  channels: ReleaseChannel[];
  rollouts: Rollout[];
  busy: string;
  act: Act;
};

export function Releases({ runtimes, channels, rollouts, busy, act }: Props) {
  return (
    <div className="grid gap-4">
      <RolloutCard rollouts={rollouts} channels={channels} runtimes={runtimes} busy={busy} act={act} />
      <ChannelsCard channels={channels} runtimes={runtimes} busy={busy} act={act} />
      <RuntimesCard runtimes={runtimes} channels={channels} busy={busy} act={act} />
    </div>
  );
}

const ROLLOUT_TONE: Record<Rollout["status"], string> = {
  rolling: "border-pending/40 bg-pending/10 text-pending",
  paused: "text-muted-foreground",
  halted: "border-destructive/40 bg-destructive/10 text-destructive",
  completed: "border-success/40 bg-success/10 text-success",
  superseded: "text-muted-foreground",
};

function RolloutBadge({ status }: { status: Rollout["status"] }) {
  return <Badge variant="outline" className={ROLLOUT_TONE[status]}>{status}</Badge>;
}

const IN_FLIGHT = new Set(["queued", "building", "transferring", "migrating", "switching"]);

/** Where one runtime stands in a rollout, as a short label and a tone. */
function runtimeProgress(runtime: Runtime, rollout: Rollout): { label: string; tone: string; title?: string } {
  const deployment = rollout.deployments.find((d) => d.runtime_id === runtime.id);
  if (runtime.deployed_ref === rollout.git_sha) {
    const unhealthy = deployment?.tenants_unhealthy ?? 0;
    return unhealthy > 0
      ? {
        label: `${unhealthy} unhealthy`,
        tone: "text-destructive",
        title: `${unhealthy} of ${deployment?.tenants_checked ?? "?"} tenants did not answer healthy after the release.`,
      }
      : { label: "released", tone: "text-success" };
  }
  if (rollout.ahead.includes(runtime.id)) {
    return { label: "newer", tone: "text-success", title: "Deployed a newer build by hand." };
  }
  if (!runtime.auto_deploy_enabled) {
    return { label: "pinned", tone: "text-muted-foreground", title: "Takes no automatic release." };
  }
  if (runtime.tenant_count === 0) {
    return { label: "no tenants", tone: "text-muted-foreground", title: "A release is checked against a served tenant." };
  }
  if (deployment && IN_FLIGHT.has(deployment.status)) return { label: deployment.status, tone: "text-pending" };
  if (deployment && (deployment.status === "failed" || deployment.status === "rolled_back")) {
    return { label: deployment.status.replace("_", " "), tone: "text-destructive" };
  }
  return { label: "waiting", tone: "text-muted-foreground" };
}

/** Where a rollout stands in one channel. */
function stageProgress(
  rollout: Rollout,
  channel: ReleaseChannel,
  members: Runtime[],
  last: boolean,
  now: number,
): { label: string; detail?: string } {
  const stage = rollout.stages.find((s) => s.channel_id === channel.id);
  const takingPart = members.filter((r) => r.auto_deploy_enabled && r.tenant_count > 0);
  if (takingPart.length === 0) return { label: "skipped", detail: "No runtime here takes automatic releases." };
  if (stage?.completed_at) {
    if (last || stage.promoted_at) return { label: "done" };
    const soaked = new Date(stage.completed_at).getTime() + channel.soak_minutes * 60_000;
    return soaked > now
      ? { label: "soaking", detail: `until ${absoluteTime(new Date(soaked).toISOString())}` }
      : { label: "done" };
  }
  if (stage) return { label: "releasing" };
  return { label: "waiting" };
}

function RolloutCard({
  rollouts,
  channels,
  runtimes,
  busy,
  act,
}: {
  rollouts: Rollout[];
  channels: ReleaseChannel[];
  runtimes: Runtime[];
  busy: string;
  act: Act;
}) {
  const current = rollouts.find(isActiveRollout) ?? rollouts[0];
  const history = rollouts.filter((rollout) => rollout !== current);
  const now = Date.now();
  return (
    <Card className="gap-0 overflow-hidden p-0">
      <CardHeader className="flex flex-col gap-3 px-4 py-3 lg:flex-row lg:items-start lg:justify-between">
        <div className="grid gap-1">
          <CardTitle className="text-base">Rollout</CardTitle>
          <CardDescription className="text-xs">
            Each new build reaches the channels in order. A channel&rsquo;s runtimes take it one at a time, every
            tenant is checked, and the channel soaks before the next one starts. Repeated failures halt the rollout.
          </CardDescription>
        </div>
        {current && isActiveRollout(current) && (
          <div className="flex flex-wrap gap-2">
            {(current.status === "rolling" || current.status === "halted") && (
              <ActionButton
                icon={<Pause />}
                busy={busy === `pause-${current.id}`}
                onClick={() => act(`/rollouts/${current.id}/pause`, `pause-${current.id}`)}
              >
                Pause
              </ActionButton>
            )}
            {(current.status === "paused" || current.status === "halted") && (
              <ActionButton
                icon={<Play />}
                busy={busy === `resume-${current.id}`}
                onClick={() => act(`/rollouts/${current.id}/resume`, `resume-${current.id}`)}
              >
                {current.status === "halted" ? "Resume with fresh attempts" : "Resume"}
              </ActionButton>
            )}
            {current.status === "rolling" && (
              <ActionButton
                icon={<FastForward />}
                busy={busy === `promote-${current.id}`}
                confirm={{
                  title: "Promote past the current channel?",
                  description:
                    "The rollout stops waiting for this channel's soak and for its unhealthy tenants, and goes on to the next runtime or channel.",
                  action: "Promote",
                }}
                onClick={() => act(`/rollouts/${current.id}/promote`, `promote-${current.id}`)}
              >
                Promote
              </ActionButton>
            )}
          </div>
        )}
      </CardHeader>
      <Separator />
      {!current ? (
        <EmptyState icon={<Rocket />}>No build has been rolled out yet. The next CI build of main starts one.</EmptyState>
      ) : (
        <div className="grid gap-3 px-4 py-3">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-mono text-sm font-semibold" title={current.git_sha}>{shortRef(current.git_sha)}</span>
            <RolloutBadge status={current.status} />
            <span className="text-muted-foreground text-xs" title={absoluteTime(current.created_at)}>
              started {relativeTime(current.created_at, now)}
            </span>
          </div>
          {current.reason && (
            <p className={cn("text-xs", current.status === "halted" ? "text-destructive" : "text-muted-foreground")}>
              {current.reason}
            </p>
          )}
          <ol className="grid gap-2 md:grid-cols-[repeat(auto-fit,minmax(14rem,1fr))]">
            {channels.map((channel, index) => {
              const members = runtimes.filter((runtime) => runtime.channel_id === channel.id);
              const stage = stageProgress(current, channel, members, index === channels.length - 1, now);
              const promoted = current.stages.find((s) => s.channel_id === channel.id)?.promoted_at;
              return (
                <li key={channel.id} className="border-border bg-muted/30 grid content-start gap-2 rounded-md border p-3">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-sm font-semibold">
                      <span className="text-muted-foreground mr-1 tabular-nums">{index + 1}.</span>
                      {channel.name}
                    </span>
                    <span className="flex gap-1">
                      {promoted && <Badge variant="outline" className="text-[10px]">promoted</Badge>}
                      <Badge variant="secondary" className="text-[10px]">{stage.label}</Badge>
                    </span>
                  </div>
                  {stage.detail && <p className="text-muted-foreground text-xs">{stage.detail}</p>}
                  {members.length === 0 ? (
                    <p className="text-muted-foreground text-xs">No runtimes.</p>
                  ) : (
                    <ul className="grid gap-1">
                      {members.map((runtime) => {
                        const progress = runtimeProgress(runtime, current);
                        return (
                          <li key={runtime.id} className="flex items-center justify-between gap-2 text-xs">
                            <span className="truncate">{runtime.name}</span>
                            <span className={cn("shrink-0", progress.tone)} title={progress.title}>{progress.label}</span>
                          </li>
                        );
                      })}
                    </ul>
                  )}
                </li>
              );
            })}
          </ol>
          {history.length > 0 && (
            <div className="grid gap-1">
              <h3 className="text-muted-foreground text-[10px] tracking-[0.08em] uppercase">Earlier builds</h3>
              <ul className="grid gap-1">
                {history.map((rollout) => (
                  <li key={rollout.id} className="flex flex-wrap items-center gap-2 text-xs">
                    <span className="font-mono" title={rollout.git_sha}>{shortRef(rollout.git_sha)}</span>
                    <RolloutBadge status={rollout.status} />
                    <span className="text-muted-foreground" title={absoluteTime(rollout.created_at)}>
                      {relativeTime(rollout.created_at, now)}
                    </span>
                    {rollout.reason && rollout.status !== "completed" && (
                      <span className="text-muted-foreground truncate">{rollout.reason}</span>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
    </Card>
  );
}

function ChannelDialog({
  channel,
  busy,
  act,
}: {
  channel?: ReleaseChannel;
  busy: string;
  act: Act;
}) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState(channel?.name ?? "");
  const [soak, setSoak] = useState(String(channel?.soak_minutes ?? 60));
  const key = channel ? `edit-channel-${channel.id}` : "create-channel";

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    const body = { name, soak_minutes: Number(soak) };
    const ok = channel
      ? await act(`/release-channels/${channel.id}`, key, body, "PUT")
      : await act("/release-channels", key, body);
    if (ok) {
      setOpen(false);
      if (!channel) {
        setName("");
        setSoak("60");
      }
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (next && channel) {
          setName(channel.name);
          setSoak(String(channel.soak_minutes));
        }
      }}
    >
      {channel ? (
        <Tooltip>
          <TooltipTrigger render={<span className="inline-flex" />}>
            <DialogTrigger render={<Button variant="outline" size="icon-sm" aria-label={`Edit ${channel.name}`} />}>
              <Pencil />
            </DialogTrigger>
          </TooltipTrigger>
          <TooltipContent>Edit</TooltipContent>
        </Tooltip>
      ) : (
        <DialogTrigger render={<Button />}>
          <Plus />
          New channel
        </DialogTrigger>
      )}
      <DialogContent>
        <form onSubmit={submit} className="grid gap-5">
          <DialogHeader>
            <DialogTitle>{channel ? `Edit ${channel.name}` : "New release channel"}</DialogTitle>
            <DialogDescription>
              {channel
                ? "The soak is how long this channel runs a release, healthy, before the next channel gets it."
                : "A new channel goes last in the rollout order. Move it earlier once it has runtimes."}
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-4">
            <div className="grid gap-2">
              <Label htmlFor={`${key}-name`}>Name</Label>
              <Input
                id={`${key}-name`}
                value={name}
                onChange={(event) => setName(event.target.value.toLowerCase())}
                placeholder="early"
                pattern="[a-z0-9][a-z0-9-]{0,31}"
                required
              />
            </div>
            <div className="grid gap-2">
              <Label htmlFor={`${key}-soak`}>Soak (minutes)</Label>
              <Input
                id={`${key}-soak`}
                type="number"
                min={0}
                max={10080}
                value={soak}
                onChange={(event) => setSoak(event.target.value)}
                required
              />
              <p className="text-muted-foreground text-xs">Up to 7 days. The last channel never soaks.</p>
            </div>
          </div>
          <DialogFooter>
            <Button type="submit" disabled={busy === key}>{channel ? "Save" : "Create channel"}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function ChannelsCard({
  channels,
  runtimes,
  busy,
  act,
}: {
  channels: ReleaseChannel[];
  runtimes: Runtime[];
  busy: string;
  act: Act;
}) {
  const unassigned = runtimes.filter((runtime) => runtime.channel_id === null);
  return (
    <Card className="gap-0 overflow-hidden p-0">
      <CardHeader className="flex flex-col gap-3 px-4 py-3 lg:flex-row lg:items-center lg:justify-between">
        <div>
          <CardTitle className="text-base">Release channels</CardTitle>
          <CardDescription className="text-xs">
            Channels group runtimes, and a tenant is in its runtime&rsquo;s channel. Move a tenant to another runtime from
            the tenant inventory to change its channel.
          </CardDescription>
        </div>
        <ChannelDialog busy={busy} act={act} />
      </CardHeader>
      <Separator />
      {channels.length === 0 ? (
        <EmptyState icon={<Layers />}>No channels. Without one, runtimes are only released by hand.</EmptyState>
      ) : (
        <ol>
          {channels.map((channel, index) => (
            <li
              key={channel.id}
              className={cn(
                "grid items-center gap-3 px-4 py-2.5 sm:grid-cols-[1fr_auto]",
                index > 0 && "border-border border-t",
              )}
            >
              <div className="min-w-0">
                <h3 className="text-sm font-semibold">
                  <span className="text-muted-foreground mr-1 tabular-nums">{index + 1}.</span>
                  {channel.name}
                </h3>
                <p className="text-muted-foreground text-xs">
                  {index === channels.length - 1
                    ? "Last; no soak"
                    : `Soaks ${channel.soak_minutes} minute${channel.soak_minutes === 1 ? "" : "s"}`}
                  {" · "}
                  {channel.runtime_count} runtime{channel.runtime_count === 1 ? "" : "s"}
                </p>
              </div>
              <div className="flex items-center gap-1">
                <ActionButton
                  iconOnly
                  icon={<ArrowUp />}
                  disabled={index === 0}
                  busy={busy === `earlier-${channel.id}`}
                  onClick={() =>
                    act(`/release-channels/${channel.id}/reorder`, `earlier-${channel.id}`, { direction: "earlier" })}
                >
                  Earlier in the rollout
                </ActionButton>
                <ActionButton
                  iconOnly
                  icon={<ArrowDown />}
                  disabled={index === channels.length - 1}
                  busy={busy === `later-${channel.id}`}
                  onClick={() =>
                    act(`/release-channels/${channel.id}/reorder`, `later-${channel.id}`, { direction: "later" })}
                >
                  Later in the rollout
                </ActionButton>
                <ChannelDialog channel={channel} busy={busy} act={act} />
                <ActionButton
                  iconOnly
                  variant="destructive"
                  icon={<Trash2 />}
                  disabled={channel.runtime_count > 0}
                  disabledReason="Move this channel's runtimes to another channel first."
                  busy={busy === `delete-channel-${channel.id}`}
                  confirm={{
                    title: `Delete ${channel.name}?`,
                    description: "The channel is removed from the rollout order. Rollouts already past it are unaffected.",
                    action: "Delete channel",
                  }}
                  onClick={() =>
                    act(`/release-channels/${channel.id}`, `delete-channel-${channel.id}`, undefined, "DELETE")}
                >
                  Delete
                </ActionButton>
              </div>
            </li>
          ))}
        </ol>
      )}
      {unassigned.length > 0 && (
        <p className="text-muted-foreground border-border border-t px-4 py-2 text-xs">
          In no channel, so released by hand only: {unassigned.map((runtime) => runtime.name).join(", ")}.
        </p>
      )}
    </Card>
  );
}

function RuntimesCard({
  runtimes,
  channels,
  busy,
  act,
}: {
  runtimes: Runtime[];
  channels: ReleaseChannel[];
  busy: string;
  act: Act;
}) {
  const [openLog, setOpenLog] = useState<string | null>(null);

  return (
    <Card className="gap-0 overflow-hidden p-0">
      <CardHeader className="px-4 py-3">
        <CardTitle className="text-base">Runtimes</CardTitle>
        <CardDescription className="text-xs">
          Eunha ships once per shared runtime, never per tenant, and only one runtime deploys at a time. A runtime
          that does not follow rollouts stays on its release until it is deployed by hand.
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
          return (
            <div key={runtime.id} className={cn(index > 0 && "border-border border-t")}>
              <div className="grid items-center gap-3 px-4 py-3 lg:grid-cols-[minmax(11rem,1fr)_minmax(14rem,1.4fr)_auto_auto_auto]">
                <div className="min-w-0">
                  <h3 className="text-sm font-semibold">{runtime.name}</h3>
                  <p className="text-muted-foreground mt-0.5 text-xs break-words">
                    {runtime.host_name} ({runtime.host_address}) · {runtime.tenant_count} tenant
                    {runtime.tenant_count === 1 ? "" : "s"}
                  </p>
                  <p
                    className="text-muted-foreground mt-0.5 font-mono text-xs"
                    title={runtime.deployed_ref || undefined}
                  >
                    {shortRef(runtime.deployed_ref)}
                  </p>
                </div>
                <div className="grid min-w-0 justify-items-start gap-1">
                  <StatusBadge status={status} />
                  <p className="text-muted-foreground text-xs break-words">
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
                <div className="grid gap-1">
                  <Label htmlFor={`channel-${runtime.id}`} className="text-muted-foreground text-xs">
                    Channel
                  </Label>
                  <NativeSelect
                    id={`channel-${runtime.id}`}
                    className="min-w-28"
                    value={runtime.channel_id ?? ""}
                    disabled={busy === `channel-${runtime.id}`}
                    onChange={(event) =>
                      act(`/runtimes/${runtime.id}/channel`, `channel-${runtime.id}`, {
                        channel_id: event.target.value || null,
                      })}
                  >
                    <option value="">None</option>
                    {channels.map((channel) => <option key={channel.id} value={channel.id}>{channel.name}</option>)}
                  </NativeSelect>
                </div>
                <div className="flex items-center gap-2">
                  <Switch
                    id={`auto-${runtime.id}`}
                    checked={runtime.auto_deploy_enabled}
                    disabled={busy === `auto-${runtime.id}`}
                    onCheckedChange={(enabled) =>
                      act(`/runtimes/${runtime.id}/automatic-release`, `auto-${runtime.id}`, {
                        enabled,
                      })}
                  />
                  <Label htmlFor={`auto-${runtime.id}`} className="text-muted-foreground text-xs">
                    Follows rollouts
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
                    description: `This deploys the newest build to every tenant on ${runtime.name} (${runtime.tenant_count} tenant${runtime.tenant_count === 1 ? "" : "s"}) on ${runtime.host_address} now, ahead of any rollout. The runtime restarts, and a failed health check restores the previous release.`,
                    action: "Deploy now",
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
  );
}
