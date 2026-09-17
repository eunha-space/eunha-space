import type { ReactNode } from "react";
import { CircleAlert, CircleCheck, CircleX, Loader } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { cn } from "cn";

export function Spinner({ label, className }: { label: string; className?: string }) {
  return <Loader className={cn("animate-spin", className)} aria-label={label} />;
}

const DOT_TONE: Record<string, string> = {
  healthy: "bg-success ring-success/25",
  unhealthy: "bg-destructive ring-destructive/25",
  disabled: "bg-transparent ring-muted-foreground/40 border-2 border-muted-foreground",
  unknown: "bg-muted-foreground ring-muted-foreground/20",
};

export function StatusDot({ state }: { state: string }) {
  return (
    <span
      role="img"
      aria-label={`Observed state: ${state}`}
      className={cn("size-2.5 shrink-0 rounded-full ring-3", DOT_TONE[state] ?? DOT_TONE.unknown)}
    />
  );
}

const STATUS_ICON: Record<string, typeof CircleCheck> = {
  succeeded: CircleCheck,
  failed: CircleX,
  running: Loader,
};

const STATUS_TONE: Record<string, string> = {
  succeeded: "border-success/40 bg-success/10 text-success",
  failed: "border-destructive/40 bg-destructive/10 text-destructive",
  running: "border-pending/40 bg-pending/10 text-pending",
};

/** Operation status: succeeded, failed, running, or an unrecognised value. */
export function StatusBadge({ status }: { status: string }) {
  const Icon = STATUS_ICON[status] ?? CircleAlert;
  return (
    <Badge variant="outline" className={cn("gap-1", STATUS_TONE[status] ?? "text-muted-foreground")}>
      <Icon className={status === "running" ? "animate-spin" : undefined} />
      {status}
    </Badge>
  );
}

export function EmptyState({ icon, children }: { icon: ReactNode; children: ReactNode }) {
  return (
    <div className="text-muted-foreground grid justify-items-center gap-2 p-8 text-center text-sm [&_svg]:size-6 [&_svg]:opacity-70">
      {icon}
      <p>{children}</p>
    </div>
  );
}
