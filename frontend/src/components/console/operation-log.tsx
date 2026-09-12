import { useEffect, useRef, useState } from "react";
import { Copy, Terminal } from "lucide-react";
import { api, errorMessage, type OperationEvent } from "@/api";
import { clockTime } from "@/format";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "./status";

/**
 * Streams `/api/operations/{id}/events`, the per-operation log the console
 * previously never surfaced. Polling stops as soon as the operation settles.
 */
export function OperationLog({ operationId, live }: { operationId: string; live: boolean }) {
  const [events, setEvents] = useState<OperationEvent[] | null>(null);
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const tail = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;
    const load = async () => {
      try {
        const rows = await api<OperationEvent[]>(`/operations/${operationId}/events`);
        if (!cancelled) {
          setEvents(rows);
          setError("");
        }
      } catch (failure) {
        if (!cancelled) setError(errorMessage(failure));
      }
    };
    void load();
    if (!live) return () => void (cancelled = true);
    const timer = setInterval(load, 2000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [operationId, live]);

  useEffect(() => {
    if (!live || !tail.current) return;
    const viewport = tail.current.querySelector<HTMLElement>('[data-slot="scroll-area-viewport"]');
    if (viewport) viewport.scrollTop = viewport.scrollHeight;
  }, [events, live]);

  async function copy() {
    const text = (events || []).map((e) => `${clockTime(e.created_at)}  ${e.message}`).join("\n");
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      setError("The log could not be copied to the clipboard.");
    }
  }

  if (error) return <p className="text-destructive px-5 py-4 text-sm">{error}</p>;

  if (!events) {
    return (
      <div className="grid gap-2 px-5 py-4">
        <Skeleton className="h-4 w-3/5" />
        <Skeleton className="h-4 w-4/5" />
        <Skeleton className="h-4 w-2/5" />
      </div>
    );
  }

  if (events.length === 0) {
    return <EmptyState icon={<Terminal />}>This operation recorded no log output.</EmptyState>;
  }

  return (
    <div className="px-5 py-4">
      <div className="mb-2.5 flex items-center justify-between gap-4">
        <h4 className="text-muted-foreground flex items-center gap-1.5 text-xs font-medium tracking-[0.08em] uppercase">
          <Terminal className="size-3.5" />
          Operation log
          {live && (
            <span className="bg-primary ml-1 size-1.5 animate-pulse rounded-full" aria-label="Streaming live" />
          )}
        </h4>
        <Button variant="ghost" size="sm" onClick={copy} aria-label="Copy the log">
          <Copy />
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
      <ScrollArea ref={tail} className="max-h-80">
        <ol className="grid gap-2 pr-3 font-mono text-[0.82rem] leading-relaxed">
          {events.map((event) => (
            <li key={event.id} className="flex gap-3">
              <time dateTime={event.created_at} className="text-primary shrink-0">
                {clockTime(event.created_at)}
              </time>
              <span className="break-words">{event.message}</span>
            </li>
          ))}
        </ol>
      </ScrollArea>
    </div>
  );
}
