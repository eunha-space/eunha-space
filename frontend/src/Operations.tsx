import { Fragment, useMemo, useState } from "react";
import { Activity, ChevronDown, ChevronRight } from "lucide-react";
import { isSettled, type Operation } from "@/api";
import { absoluteTime, humanize, relativeTime } from "@/format";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { ChipFilter, SearchField } from "@/components/console/filters";
import { OperationLog } from "@/components/console/operation-log";
import { EmptyState, StatusBadge } from "@/components/console/status";
import { cn } from "cn";

export function Operations({ operations }: { operations: Operation[] }) {
  const [status, setStatus] = useState("all");
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<string | null>(null);

  const counts = useMemo(() => {
    const tally: Record<string, number> = { all: operations.length };
    for (const operation of operations) {
      tally[operation.status] = (tally[operation.status] || 0) + 1;
    }
    return tally;
  }, [operations]);

  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return operations.filter((operation) => {
      if (status !== "all" && operation.status !== status) return false;
      if (needle === "") return true;
      const haystack = `${operation.domain || ""} ${operation.runtime_name || ""} ${operation.kind}`;
      return haystack.toLowerCase().includes(needle);
    });
  }, [operations, status, query]);

  return (
    <Card className="gap-0 overflow-hidden p-0">
      <CardHeader className="flex flex-col gap-3 px-4 py-3 lg:flex-row lg:items-center lg:justify-between">
        <div>
          <CardTitle className="text-base">Operations</CardTitle>
          <CardDescription className="text-xs">
            The 50 most recent releases, configuration renders, and health checks. Select a row to
            read its log.
          </CardDescription>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <SearchField
            value={query}
            onChange={setQuery}
            label="Search operations"
            placeholder="Search target or kind"
          />
          <ChipFilter
            label="Filter operations by status"
            value={status}
            onChange={setStatus}
            choices={[
              { value: "all", label: "All", count: counts.all },
              { value: "running", label: "Running", count: counts.running || 0 },
              { value: "succeeded", label: "Succeeded", count: counts.succeeded || 0 },
              { value: "failed", label: "Failed", count: counts.failed || 0 },
            ]}
          />
        </div>
      </CardHeader>
      <Separator />
      {visible.length === 0 ? (
        <EmptyState icon={<Activity />}>
          {operations.length === 0
            ? "No operations have been recorded."
            : "No operations match the current search and filter."}
        </EmptyState>
      ) : (
        <div className="overflow-x-auto">
          <Table>
            <TableCaption className="sr-only">
              Recent control-plane operations, newest first. Each row expands to its log.
            </TableCaption>
            <TableHeader>
              <TableRow>
                <TableHead className="w-10">
                  <span className="sr-only">Expand</span>
                </TableHead>
                <TableHead>Operation</TableHead>
                <TableHead>Target</TableHead>
                <TableHead>Status</TableHead>
                <TableHead className="hidden lg:table-cell">Detail</TableHead>
                <TableHead className="hidden md:table-cell">When</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {visible.map((operation) => {
                const expanded = open === operation.id;
                return (
                  <Fragment key={operation.id}>
                    <TableRow
                      className={cn(
                        "cursor-pointer",
                        expanded && "bg-accent/50",
                        // The detail row below carries the border on narrow screens.
                        operation.error && "max-lg:border-b-0",
                      )}
                      onClick={() => setOpen(expanded ? null : operation.id)}
                    >
                      <TableCell className="pr-0">
                        <button
                          type="button"
                          className="text-muted-foreground hover:text-foreground cursor-pointer"
                          aria-expanded={expanded}
                          aria-label={`${expanded ? "Hide" : "Show"} the log for ${humanize(operation.kind)}`}
                          onClick={(event) => {
                            event.stopPropagation();
                            setOpen(expanded ? null : operation.id);
                          }}
                        >
                          {expanded ? <ChevronDown className="size-4" /> : <ChevronRight className="size-4" />}
                        </button>
                      </TableCell>
                      {/* Below lg the detail and timestamp fold into this first cell. */}
                      <TableCell className="align-top whitespace-nowrap">
                        {humanize(operation.kind)}
                        <span
                          className="text-muted-foreground block text-xs font-normal md:hidden"
                          title={absoluteTime(operation.created_at)}
                        >
                          {relativeTime(operation.created_at)}
                        </span>
                      </TableCell>
                      <TableCell className="align-top">
                        {operation.domain || operation.runtime_name || "—"}
                      </TableCell>
                      <TableCell className="align-top">
                        <StatusBadge status={operation.status} />
                      </TableCell>
                      <TableCell className="text-muted-foreground hidden max-w-md align-top text-wrap break-words whitespace-normal lg:table-cell">
                        {operation.error || "—"}
                      </TableCell>
                      <TableCell
                        className="hidden align-top whitespace-nowrap md:table-cell"
                        title={absoluteTime(operation.created_at)}
                      >
                        {relativeTime(operation.created_at)}
                      </TableCell>
                    </TableRow>
                    {operation.error && (
                      <TableRow
                        className={cn("cursor-pointer lg:hidden", expanded && "bg-accent/50")}
                        onClick={() => setOpen(expanded ? null : operation.id)}
                      >
                        <TableCell />
                        <TableCell
                          colSpan={4}
                          className="text-destructive pt-0 pr-4 text-xs text-wrap whitespace-normal"
                        >
                          {operation.error}
                        </TableCell>
                      </TableRow>
                    )}
                    {expanded && (
                      <TableRow className="hover:bg-transparent">
                        <TableCell colSpan={6} className="bg-muted/40 p-0">
                          <OperationLog
                            operationId={operation.id}
                            live={!isSettled(operation.status)}
                          />
                        </TableCell>
                      </TableRow>
                    )}
                  </Fragment>
                );
              })}
            </TableBody>
          </Table>
        </div>
      )}
    </Card>
  );
}
