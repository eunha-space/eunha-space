import { useMemo } from "react";
import { HardDrive, Server } from "lucide-react";
import type { Runtime, Tenant } from "@/api";
import { shortRef } from "@/format";
import { Badge } from "@/components/ui/badge";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { EmptyState } from "@/components/console/status";

/**
 * The physical fleet: each instance and the shared runtimes it carries. An
 * instance is a host row in the control plane, so several runtimes can
 * legitimately share one.
 */
export function Instances({ runtimes, tenants }: { runtimes: Runtime[]; tenants: Tenant[] }) {
  const instances = useMemo(() => {
    const grouped = new Map<string, Runtime[]>();
    for (const runtime of runtimes) {
      const existing = grouped.get(runtime.instance_name);
      if (existing) existing.push(runtime);
      else grouped.set(runtime.instance_name, [runtime]);
    }
    return [...grouped.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [runtimes]);

  const tenantsByRuntime = useMemo(() => {
    const grouped = new Map<string, Tenant[]>();
    for (const tenant of tenants) {
      const existing = grouped.get(tenant.runtime_name);
      if (existing) existing.push(tenant);
      else grouped.set(tenant.runtime_name, [tenant]);
    }
    return grouped;
  }, [tenants]);

  if (instances.length === 0) {
    return (
      <Card className="p-0">
        <EmptyState icon={<HardDrive />}>No instances are registered.</EmptyState>
      </Card>
    );
  }

  return (
    <div className="grid gap-4">
      {instances.map(([instanceName, instanceRuntimes]) => {
        const instanceTenants = instanceRuntimes.reduce(
          (total, runtime) => total + runtime.tenant_count,
          0,
        );
        const instanceAddress = instanceRuntimes[0]?.instance_address;
        return (
          <Card key={instanceName} className="gap-0 overflow-hidden p-0">
            <CardHeader className="flex flex-row items-center gap-3 p-5">
              <HardDrive className="text-muted-foreground" />
              <div className="min-w-0 flex-1">
                <CardTitle className="font-mono">{instanceName}</CardTitle>
                <CardDescription>
                  {instanceAddress} · {instanceRuntimes.length} runtime
                  {instanceRuntimes.length === 1 ? "" : "s"} · {instanceTenants} tenant
                  {instanceTenants === 1 ? "" : "s"}
                </CardDescription>
              </div>
            </CardHeader>
            <Separator />
            {instanceRuntimes.map((runtime, index) => (
              <div
                key={runtime.id}
                className={index > 0 ? "border-border border-t p-5" : "p-5"}
              >
                <div className="mb-3 flex flex-wrap items-center gap-2">
                  <Server className="text-muted-foreground size-4" />
                  <h3 className="font-semibold">{runtime.name}</h3>
                  <Badge variant="outline" className="text-muted-foreground font-mono">
                    {runtime.bind_address}
                  </Badge>
                  {runtime.auto_deploy_enabled && (
                    <Badge variant="outline" className="border-primary/40 bg-primary/10 text-primary">
                      Auto deploy
                    </Badge>
                  )}
                </div>
                <dl className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
                  <Detail label="Service label" value={runtime.service_label} mono />
                  <Detail label="Tenant directory" value={runtime.tenant_directory} mono />
                  <Detail label="Cloudflare origin" value={runtime.origin_hostname} mono />
                  <Detail label="Router" value={runtime.router_enabled ? `${runtime.bind_address} → blue/green` : "legacy restart"} mono />
                  <Detail
                    label="Deployed release"
                    value={shortRef(runtime.deployed_ref)}
                    title={runtime.deployed_ref || undefined}
                    mono
                  />
                  <Detail
                    label="Tenants"
                    value={
                      tenantsByRuntime
                        .get(runtime.name)
                        ?.map((tenant) => tenant.domain)
                        .join(", ") || "none"
                    }
                  />
                </dl>
              </div>
            ))}
          </Card>
        );
      })}
    </div>
  );
}

function Detail({
  label,
  value,
  mono = false,
  title,
}: {
  label: string;
  value: string;
  mono?: boolean;
  title?: string;
}) {
  return (
    <div className="min-w-0">
      <dt className="text-muted-foreground text-xs tracking-[0.08em] uppercase">{label}</dt>
      <dd className={`mt-0.5 break-words ${mono ? "font-mono text-xs" : "text-sm"}`} title={title}>
        {value}
      </dd>
    </div>
  );
}
