import { Fragment, useCallback, useEffect, useMemo, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  CircleAlert,
  CreditCard,
  ExternalLink,
  RotateCcw,
} from "lucide-react";
import { api, errorMessage } from "@/api";
import { absoluteTime, relativeTime } from "@/format";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { ActionButton } from "@/components/console/action-button";
import { ChipFilter, SearchField } from "@/components/console/filters";
import { EmptyState, Spinner } from "@/components/console/status";
import { cn } from "cn";

type Status = "incomplete" | "active" | "past_due" | "canceled" | "lapsed";

type Account = {
  user_id: string;
  email: string | null;
  is_admin: boolean;
  billing_exempt: boolean;
  status: Status | null;
  amount: number | null;
  card_company: string | null;
  card_number: string | null;
  has_card: boolean;
  current_period_end: string | null;
  next_attempt_at: string | null;
  cancel_at_period_end: boolean;
  failed_attempts: number;
  last_error: string | null;
  instances: number;
  suspended_instances: number;
  refund_open: boolean;
  last_paid_at: string | null;
};

type Overview = {
  enabled: boolean;
  amount: number | null;
  summary: {
    active: number;
    past_due: number;
    canceling: number;
    ended: number;
    monthly_revenue: number;
    unbilled: number;
    refunds_open: number;
    keys_waiting: number;
  };
  accounts: Account[];
};

type Payment = {
  order_id: string;
  amount: number;
  status: "pending" | "paid" | "failed" | "expired" | "refunded";
  period_start: string;
  period_end: string;
  requested_at: string;
  settled_at: string | null;
  receipt_url: string | null;
  failure_code: string | null;
  failure_message: string | null;
  refunded_amount: number;
  refund_pending: boolean;
  refund_error: string | null;
  refundable: boolean;
};

type AccountDetail = {
  payments: Payment[];
  retired_keys: {
    retired_at: string;
    deleted_at: string | null;
    attempts: number;
    last_error: string | null;
  }[];
};

type Filter = "all" | "attention" | "active" | "canceling" | "ended" | "unbilled" | "exempt";

function won(amount: number): string {
  return new Intl.NumberFormat("ko-KR", { style: "currency", currency: "KRW" }).format(amount);
}

function day(iso: string | null): string {
  if (!iso) return "—";
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleDateString();
}

/** What an account's billing state is, in one word an operator can scan. */
function standing(account: Account): { label: string; tone: string } {
  if (account.billing_exempt || account.is_admin) {
    return { label: account.is_admin ? "admin" : "exempt", tone: "text-muted-foreground" };
  }
  switch (account.status) {
    case "active":
      return account.cancel_at_period_end
        ? { label: "canceling", tone: "text-pending" }
        : { label: "active", tone: "text-success" };
    case "past_due":
      return { label: "past due", tone: "text-destructive" };
    case "incomplete":
      return { label: "incomplete", tone: "text-pending" };
    case "canceled":
    case "lapsed":
      return { label: account.status, tone: "text-muted-foreground" };
    default:
      return { label: "unbilled", tone: "text-pending" };
  }
}

function matches(account: Account, filter: Filter): boolean {
  switch (filter) {
    case "attention":
      return account.status === "past_due" || account.refund_open ||
        (account.status === "incomplete" && account.last_error != null);
    case "active":
      return account.status === "active" && !account.cancel_at_period_end;
    case "canceling":
      return account.status === "active" && account.cancel_at_period_end;
    case "ended":
      return account.status === "canceled" || account.status === "lapsed";
    case "unbilled":
      return account.status !== "active" && account.status !== "past_due" &&
        !account.billing_exempt && !account.is_admin && account.instances > 0;
    case "exempt":
      return account.billing_exempt;
    default:
      return true;
  }
}

export function Billing() {
  const [overview, setOverview] = useState<Overview | null>(null);
  const [error, setError] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setOverview(await api<Overview>("/billing"));
      setError("");
    } catch (failure) {
      setError(errorMessage(failure));
    }
  }, []);

  // Refreshed like the rest of the console, while the tab is shown.
  useEffect(() => {
    void load();
    const timer = setInterval(() => {
      if (!document.hidden) void load();
    }, 15000);
    return () => clearInterval(timer);
  }, [load]);

  const accounts = overview?.accounts ?? [];
  const counts = useMemo(() => {
    const tally = {} as Record<Filter, number>;
    for (const key of ["all", "attention", "active", "canceling", "ended", "unbilled", "exempt"] as Filter[]) {
      tally[key] = accounts.filter((account) => matches(account, key)).length;
    }
    return tally;
  }, [accounts]);
  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return accounts.filter((account) =>
      matches(account, filter) &&
      (needle === "" || (account.email ?? "").toLowerCase().includes(needle))
    );
  }, [accounts, filter, query]);

  if (!overview) {
    return error
      ? (
        <Alert variant="destructive">
          <CircleAlert />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )
      : <Spinner label="Loading billing" className="size-5" />;
  }

  const summary = overview.summary;
  const tiles: [string, string, boolean][] = [
    ["Monthly revenue", won(summary.monthly_revenue), false],
    ["Active", String(summary.active), false],
    ["Past due", String(summary.past_due), summary.past_due > 0],
    ["Canceling", String(summary.canceling), false],
    ["Unbilled with instances", String(summary.unbilled), false],
    ["Refunds open", String(summary.refunds_open), summary.refunds_open > 0],
    ["Keys to delete", String(summary.keys_waiting), summary.keys_waiting > 0],
  ];

  return (
    <div className="grid gap-4">
      {!overview.enabled && (
        <Alert>
          <CreditCard />
          <AlertDescription>
            Billing is off: set TOSSPAYMENTS_CLIENT_KEY, TOSSPAYMENTS_SECRET_KEY and
            EUNHA_SPACE_SUBSCRIPTION_AMOUNT on the control plane to start charging. Until then
            nobody needs a subscription and customers see no subscription card.
          </AlertDescription>
        </Alert>
      )}
      {error && (
        <Alert variant="destructive">
          <CircleAlert />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      <Card className="grid grid-cols-2 gap-0 overflow-hidden p-0 md:grid-cols-4 xl:grid-cols-7">
        {tiles.map(([label, value, alarming], index) => (
          <div
            key={label}
            className={cn(
              "border-border px-4 py-2.5",
              index < tiles.length - 1 && "border-b xl:border-b-0 xl:border-r",
            )}
          >
            <span className="text-muted-foreground block text-xs">{label}</span>
            <strong
              className={cn(
                "block text-xl font-semibold tabular-nums",
                alarming && "text-destructive",
              )}
            >
              {value}
            </strong>
          </div>
        ))}
      </Card>

      <Card className="gap-0 overflow-hidden p-0">
        <CardHeader className="flex flex-col gap-3 px-4 py-3">
          <div>
            <CardTitle className="text-base">Subscribers</CardTitle>
            <CardDescription className="text-xs">
              {overview.amount != null
                ? `New subscriptions are ${won(overview.amount)} a month. `
                : ""}
              Accounts that subscribed, are exempt, or run instances unbilled. Select a row for its
              payments.
            </CardDescription>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <SearchField
              value={query}
              onChange={setQuery}
              label="Search accounts"
              placeholder="Search email"
            />
            <ChipFilter
              label="Filter accounts"
              value={filter}
              onChange={(value) => setFilter(value as Filter)}
              choices={[
                { value: "all", label: "All", count: counts.all },
                { value: "attention", label: "Needs attention", count: counts.attention },
                { value: "active", label: "Active", count: counts.active },
                { value: "canceling", label: "Canceling", count: counts.canceling },
                { value: "ended", label: "Ended", count: counts.ended },
                { value: "unbilled", label: "Unbilled", count: counts.unbilled },
                { value: "exempt", label: "Exempt", count: counts.exempt },
              ]}
            />
          </div>
        </CardHeader>
        <Separator />
        {visible.length === 0
          ? (
            <EmptyState icon={<CreditCard />}>
              {accounts.length === 0
                ? "No account has subscribed yet."
                : "No accounts match the current search and filter."}
            </EmptyState>
          )
          : (
            <div className="overflow-x-auto">
              <Table>
                <TableCaption className="sr-only">
                  Billing accounts. Each row expands to its payments.
                </TableCaption>
                <TableHeader>
                  <TableRow>
                    <TableHead className="w-10">
                      <span className="sr-only">Expand</span>
                    </TableHead>
                    <TableHead>Account</TableHead>
                    <TableHead>Standing</TableHead>
                    <TableHead className="text-right">Monthly</TableHead>
                    <TableHead className="hidden md:table-cell">Renews / ends</TableHead>
                    <TableHead className="hidden lg:table-cell">Card</TableHead>
                    <TableHead className="text-right">Instances</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {visible.map((account) => {
                    const expanded = open === account.user_id;
                    const { label, tone } = standing(account);
                    return (
                      <Fragment key={account.user_id}>
                        <TableRow
                          className={cn("cursor-pointer", expanded && "bg-accent/50")}
                          onClick={() => setOpen(expanded ? null : account.user_id)}
                        >
                          <TableCell className="pr-0">
                            <button
                              type="button"
                              className="text-muted-foreground hover:text-foreground cursor-pointer"
                              aria-expanded={expanded}
                              aria-label={`${expanded ? "Hide" : "Show"} payments for ${account.email}`}
                              onClick={(event) => {
                                event.stopPropagation();
                                setOpen(expanded ? null : account.user_id);
                              }}
                            >
                              {expanded
                                ? <ChevronDown className="size-4" />
                                : <ChevronRight className="size-4" />}
                            </button>
                          </TableCell>
                          <TableCell className="align-top">
                            {account.email}
                            {account.last_error &&
                              (account.status === "past_due" ||
                                account.status === "incomplete") && (
                              <span className="text-destructive block text-xs text-wrap">
                                {account.last_error}
                              </span>
                            )}
                          </TableCell>
                          <TableCell className="align-top">
                            <Badge variant="outline" className={tone}>{label}</Badge>
                            {account.status === "past_due" && (
                              <span className="text-muted-foreground block text-xs">
                                {account.failed_attempts} declined
                                {account.next_attempt_at &&
                                  `, next ${relativeTime(account.next_attempt_at)}`}
                              </span>
                            )}
                            {account.refund_open && (
                              <span className="text-pending block text-xs">refund open</span>
                            )}
                          </TableCell>
                          <TableCell className="text-right align-top tabular-nums">
                            {account.amount != null &&
                                (account.status === "active" || account.status === "past_due")
                              ? won(account.amount)
                              : "—"}
                          </TableCell>
                          <TableCell
                            className="hidden align-top md:table-cell"
                            title={account.current_period_end
                              ? absoluteTime(account.current_period_end)
                              : undefined}
                          >
                            {account.status === "active" || account.status === "past_due"
                              ? day(account.current_period_end)
                              : "—"}
                          </TableCell>
                          <TableCell className="text-muted-foreground hidden align-top lg:table-cell">
                            {account.has_card
                              ? [account.card_company, account.card_number].filter(Boolean)
                                .join(" ")
                              : "—"}
                          </TableCell>
                          <TableCell className="text-right align-top tabular-nums">
                            {account.instances}
                            {account.suspended_instances > 0 && (
                              <span className="text-muted-foreground block text-xs">
                                {account.suspended_instances} suspended
                              </span>
                            )}
                          </TableCell>
                        </TableRow>
                        {expanded && (
                          <TableRow className="hover:bg-transparent">
                            <TableCell colSpan={7} className="bg-muted/40 p-0">
                              <AccountPanel account={account} onChanged={load} />
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
    </div>
  );
}

function AccountPanel(
  { account, onChanged }: { account: Account; onChanged: () => Promise<void> },
) {
  const [detail, setDetail] = useState<AccountDetail | null>(null);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    try {
      setDetail(await api<AccountDetail>(`/billing/accounts/${account.user_id}`));
    } catch (failure) {
      setError(errorMessage(failure));
    }
  }, [account.user_id]);

  useEffect(() => {
    void load();
  }, [load]);

  async function act(key: string, path: string, body?: unknown) {
    setBusy(key);
    setError("");
    try {
      await api(path, { method: "POST", body: body === undefined ? undefined : JSON.stringify(body) });
      await Promise.all([load(), onChanged()]);
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy("");
    }
  }

  const retryable = (account.status === "past_due" || account.status === "incomplete") &&
    account.has_card;

  return (
    <div className="grid gap-3 p-4">
      <div className="flex flex-wrap items-center gap-4">
        {retryable && (
          <ActionButton
            icon={<RotateCcw />}
            size="sm"
            busy={busy === "retry"}
            onClick={() => void act("retry", `/billing/accounts/${account.user_id}/retry`)}
            confirm={{
              title: `Charge ${account.email} again now?`,
              description:
                "The card on file is charged for the period that is due. A decline counts as one of the retries.",
              action: "Charge now",
            }}
          >
            Charge again now
          </ActionButton>
        )}
        {!account.is_admin && (
          <div className="flex items-center gap-2">
            <Switch
              id={`exempt-${account.user_id}`}
              checked={account.billing_exempt}
              disabled={busy === "exempt"}
              onCheckedChange={(exempt) =>
                void act("exempt", `/billing/accounts/${account.user_id}/exempt`, { exempt })}
            />
            <Label htmlFor={`exempt-${account.user_id}`} className="text-muted-foreground text-xs">
              Exempt from billing: runs instances without a subscription, and keeps them if one ends
            </Label>
          </div>
        )}
      </div>
      {error && (
        <Alert variant="destructive">
          <CircleAlert />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      {!detail
        ? <Spinner label="Loading payments" className="size-4" />
        : detail.payments.length === 0
        ? <p className="text-muted-foreground text-sm">No payments yet.</p>
        : (
          <div className="bg-background overflow-x-auto rounded-md border">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Requested</TableHead>
                  <TableHead>Period</TableHead>
                  <TableHead className="text-right">Amount</TableHead>
                  <TableHead>Result</TableHead>
                  <TableHead className="hidden lg:table-cell">Order</TableHead>
                  <TableHead />
                </TableRow>
              </TableHeader>
              <TableBody>
                {detail.payments.map((payment) => (
                  <TableRow key={payment.order_id}>
                    <TableCell title={absoluteTime(payment.requested_at)}>
                      {relativeTime(payment.requested_at)}
                    </TableCell>
                    <TableCell className="whitespace-nowrap">
                      {day(payment.period_start)} – {day(payment.period_end)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">{won(payment.amount)}</TableCell>
                    <TableCell className="text-wrap">
                      <PaymentResult payment={payment} />
                    </TableCell>
                    <TableCell className="text-muted-foreground hidden font-mono text-xs lg:table-cell">
                      {payment.order_id}
                    </TableCell>
                    <TableCell className="text-right">
                      {payment.refundable && (
                        <ActionButton
                          size="sm"
                          variant="ghost"
                          busy={busy === payment.order_id}
                          onClick={() =>
                            void act(
                              payment.order_id,
                              `/billing/payments/${encodeURIComponent(payment.order_id)}/refund`,
                            )}
                          confirm={{
                            title: `Refund ${won(payment.amount)} to ${account.email}?`,
                            description:
                              "The full amount goes back to the card. If this paid for the period being served, the subscription ends now and the account's instances go offline with their data kept.",
                            action: "Refund",
                          }}
                        >
                          Refund
                        </ActionButton>
                      )}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
      {detail && detail.retired_keys.some((key) => !key.deleted_at) && (
        <p className="text-muted-foreground text-xs">
          {detail.retired_keys
            .filter((key) => !key.deleted_at)
            .map((key) =>
              `A retired card key is waiting to be deleted at TossPayments (${key.attempts} tries${
                key.last_error ? `, last: ${key.last_error}` : ""
              }).`
            )
            .join(" ")}
        </p>
      )}
    </div>
  );
}

function PaymentResult({ payment }: { payment: Payment }) {
  if (payment.status === "refunded") {
    return <Badge variant="outline">refunded</Badge>;
  }
  if (payment.refund_pending) {
    return <Badge variant="outline" className="text-pending">refund pending</Badge>;
  }
  return (
    <span className="grid gap-0.5">
      <span className="flex items-center gap-1">
        <Badge
          variant="outline"
          className={cn(
            payment.status === "paid" && "text-success",
            payment.status === "failed" && "text-destructive",
            payment.status === "pending" && "text-pending",
          )}
        >
          {payment.status}
        </Badge>
        {payment.receipt_url && (
          <a
            href={payment.receipt_url}
            target="_blank"
            rel="noreferrer"
            aria-label="Receipt"
            className="text-muted-foreground hover:text-foreground"
          >
            <ExternalLink className="size-3" />
          </a>
        )}
      </span>
      {payment.failure_message && (
        <span className="text-destructive text-xs">
          {payment.failure_code ? `${payment.failure_code}: ` : ""}
          {payment.failure_message}
        </span>
      )}
      {payment.refund_error && (
        <span className="text-destructive text-xs">Refund refused: {payment.refund_error}</span>
      )}
    </span>
  );
}
