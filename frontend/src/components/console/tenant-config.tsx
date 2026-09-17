import { useEffect, useState } from "react";
import { FileCog } from "lucide-react";
import { api, errorMessage, type TenantConfiguration } from "@/api";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Skeleton } from "@/components/ui/skeleton";

/**
 * Reads `/api/tenants/{id}/configuration`, the rendered runtime file with
 * every secret withheld server-side. Loaded only while the dialog is open.
 */
export function TenantConfigDialog({ tenantId, domain }: { tenantId: string; domain: string }) {
  const [open, setOpen] = useState(false);
  const [config, setConfig] = useState<TenantConfiguration | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setError("");
    api<TenantConfiguration>(`/tenants/${tenantId}/configuration`)
      .then((next) => !cancelled && setConfig(next))
      .catch((failure) => !cancelled && setError(errorMessage(failure)));
    return () => {
      cancelled = true;
    };
  }, [open, tenantId]);

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <Tooltip>
        <TooltipTrigger render={<span className="inline-flex" />}>
          <DialogTrigger
            render={<Button variant="outline" size="icon-sm" aria-label="Config" />}
          >
            <FileCog />
          </DialogTrigger>
        </TooltipTrigger>
        <TooltipContent>Config</TooltipContent>
      </Tooltip>
      {/* The body is the only part that grows, so the dialog stays on screen
          however long the file is. */}
      <DialogContent className="max-h-[calc(100vh-4rem)] grid-rows-[auto_minmax(0,1fr)] sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle className="flex flex-wrap items-center gap-2">
            <span className="font-mono">{config?.file_name ?? `${domain} configuration`}</span>
            {config && (
              <Badge variant="outline" className="text-muted-foreground">
                revision {config.config_revision}
                {config.rendered_revision !== config.config_revision &&
                  ` · rendered ${config.rendered_revision ?? "—"}`}
              </Badge>
            )}
          </DialogTitle>
          <DialogDescription>
            What a render would write to the runtime. Secret values are withheld by the API and
            never reach this page.
          </DialogDescription>
        </DialogHeader>
        {/* A grid child defaults to min-width:auto, which let the longest line
            push the panel out past the dialog; a rendered file wraps rather
            than scrolling sideways. */}
        {error ? (
          <p className="text-destructive text-sm">{error}</p>
        ) : !config ? (
          <div className="grid gap-2">
            <Skeleton className="h-4 w-2/3" />
            <Skeleton className="h-4 w-1/2" />
            <Skeleton className="h-4 w-3/4" />
            <Skeleton className="h-4 w-1/3" />
          </div>
        ) : (
          <ScrollArea className="bg-muted/40 min-h-0 min-w-0 rounded-lg border">
            <pre className="p-4 font-mono text-xs leading-relaxed break-words whitespace-pre-wrap">{config.toml}</pre>
          </ScrollArea>
        )}
      </DialogContent>
    </Dialog>
  );
}
