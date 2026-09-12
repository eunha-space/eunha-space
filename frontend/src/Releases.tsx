import { useState } from "react";
import { ChevronDown, ChevronRight, Rocket, Server } from "lucide-react";
import { isSettled, type Runtime } from "@/api";
import { shortRef } from "@/format";
import { Button } from "@/components/ui/button";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import { ActionButton } from "@/components/console/action-button";
import { OperationLog } from "@/components/console/operation-log";
import { EmptyState, StatusBadge } from "@/components/console/status";
import { cn } from "cn";

type Props = {
  runtimes: Runtime[];
  busy: string;
  act: (path: string, key: string, body?: object) => void;
};

export function Releases({ runtimes, busy, act }: Props) {
  const [openLog, setOpenLog] = useState<string | null>(null);

  return (
    <Card className="gap-0 overflow-hidden p-0">
      <CardHeader className="p-5">
        <CardTitle>Runtime releases</CardTitle>
        <CardDescription>
          Eunha ships once per shared runtime, never per tenant. The control plane serializes the
          fleet, so only one runtime deploys at a time.
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
              <div className="grid items-center gap-4 p-5 lg:grid-cols-[minmax(11rem,1fr)_minmax(14rem,1.4fr)_auto_auto]">
                <div className="min-w-0">
                  <h3 className="font-semibold">{runtime.name}</h3>
                  <p className="text-muted-foreground mt-0.5 text-sm break-words">
                    {runtime.host} · {runtime.tenant_count} tenant
                    {runtime.tenant_count === 1 ? "" : "s"}
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
                      act(`/runtimes/${runtime.id}/automatic-release`, `auto-${runtime.id}`, {
                        enabled,
                      })
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
                    description: `This builds the current central release and rolls it out to every tenant on ${runtime.name} (${runtime.tenant_count} tenant${runtime.tenant_count === 1 ? "" : "s"}) on ${runtime.host}. The runtime restarts, and a failed health check restores the previous release.`,
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
  );
}
