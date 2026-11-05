import { won } from "@/format";
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
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
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

type Plan = {
  id: string;
  name_en: string;
  name_ko: string;
  amount: number;
  instance_limit: number | null;
  user_limit: number | null;
  media_gb: number | null;
  database_gb: number | null;
  monthly_activities: number | null;
  active: boolean;
  subscribers: number;
};

type Overview = {
  enabled: boolean;
  plans: Plan[];
  summary: {
    active: number;
    past_due: number;
    canceling: number;
    ended: number;
    monthly_revenue: number;
    unbilled: number;
    refunds_open: number;
    keys_waiting: number;
    mismatches: number;
  };
  accounts: Account[];
  mismatches: Mismatch[];
  last_check: { day: string; transactions: number | null; error: string | null } | null;
  webhooks: Webhook[];
};

type Mismatch = {
  id: string;
  order_id: string;
  kind: string;
  detail: string;
  found_at: string;
};

type Webhook = {
  id: string;
  received_at: string;
  event_type: string | null;
  order_id: string | null;
  signed: boolean;
  headers: Record<string, string>;
  payload: string | null;
  http_status: number;
};

const MISMATCH_LABELS: Record<string, string> = {
  approved_not_recorded: "Approved, but closed here",
  unknown_order: "Not in our records",
  amount_differs: "Amount differs",
  not_at_toss: "Not at TossPayments",
  still_pending: "No outcome after a day",
};

function TossRecords({ overview, onChanged }: { overview: Overview; onChanged: () => Promise<void> }) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");
  const check = overview.last_check;
  const resolve = async (id: string) => {
    setBusy(id);
    try {
      await api(`/billing/mismatches/${id}/resolve`, { method: "POST" });
      setError("");
      await onChanged();
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(null);
    }
  };
  return (
    <Card className="gap-0 overflow-hidden p-0">
      <CardHeader className="px-4 py-3">
        <CardTitle className="text-base">TossPayments records</CardTitle>
        <CardDescription className="text-xs">
          {check
            ? check.error
              ? `Comparing ${check.day} failed and is tried again hourly: ${check.error}`
              : `Last day compared: ${check.day}, ${check.transactions ?? 0} transactions.`
            : "No day has been compared yet. Each day is compared after 04:00 in Korea."}
        </CardDescription>
      </CardHeader>
      {error && (
        <Alert variant="destructive" className="mx-4 mb-3 w-auto">
          <CircleAlert />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      {overview.mismatches.length > 0 && (
        <Table>
          <TableCaption className="sr-only">Mismatches to look at</TableCaption>
          <TableHeader>
            <TableRow>
              <TableHead>Mismatch</TableHead>
              <TableHead>Order</TableHead>
              <TableHead>Detail</TableHead>
              <TableHead>Found</TableHead>
              <TableHead />
            </TableRow>
          </TableHeader>
          <TableBody>
            {overview.mismatches.map((mismatch) => (
              <TableRow key={mismatch.id}>
                <TableCell className="text-destructive font-medium">
                  {MISMATCH_LABELS[mismatch.kind] ?? mismatch.kind}
                </TableCell>
                <TableCell className="font-mono text-xs">{mismatch.order_id}</TableCell>
                <TableCell className="text-xs">{mismatch.detail}</TableCell>
                <TableCell className="text-xs">{relativeTime(mismatch.found_at)}</TableCell>
                <TableCell className="text-right">
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={busy === mismatch.id}
                    onClick={() => void resolve(mismatch.id)}
                  >
                    Mark resolved
                  </Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      <Table>
        <TableCaption className="sr-only">Recent webhooks</TableCaption>
        <TableHeader>
          <TableRow>
            <TableHead>Webhook</TableHead>
            <TableHead>Order</TableHead>
            <TableHead>Signature</TableHead>
            <TableHead>Answered</TableHead>
            <TableHead>Received</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {overview.webhooks.length === 0 && (
            <TableRow>
              <TableCell colSpan={5} className="text-muted-foreground text-xs">
                No webhooks yet.
              </TableCell>
            </TableRow>
          )}
          {overview.webhooks.map((webhook) => (
            <TableRow key={webhook.id}>
              <TableCell className="align-top text-xs">
                <details>
                  <summary className="cursor-pointer">{webhook.event_type ?? "Unreadable"}</summary>
                  <pre className="mt-2 max-w-xl overflow-x-auto text-xs whitespace-pre-wrap">
                    {JSON.stringify(webhook.headers, null, 2)}
                    {"\n"}
                    {webhook.payload}
                  </pre>
                </details>
              </TableCell>
              <TableCell className="align-top font-mono text-xs">{webhook.order_id}</TableCell>
              <TableCell className="align-top text-xs">{webhook.signed ? "Signed" : "None"}</TableCell>
              <TableCell className="align-top text-xs">{webhook.http_status}</TableCell>
              <TableCell className="align-top text-xs">{relativeTime(webhook.received_at)}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </Card>
  );
}

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

function Plans({ plans, onChanged }: { plans: Plan[]; onChanged: () => Promise<void> }) {
  const [nameEn, setNameEn] = useState("");
  const [nameKo, setNameKo] = useState("");
  const [amount, setAmount] = useState("");
  const [limit, setLimit] = useState("");
  const [userLimit, setUserLimit] = useState("");
  const [mediaGb, setMediaGb] = useState("");
  const [databaseGb, setDatabaseGb] = useState("");
  const [activities, setActivities] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function run(action: () => Promise<unknown>) {
    setBusy(true);
    setError("");
    try {
      await action();
      await onChanged();
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  }

  const create = () =>
    run(async () => {
      await api("/billing/plans", {
        method: "POST",
        body: JSON.stringify({
          name_en: nameEn,
          name_ko: nameKo,
          amount: Number(amount),
          instance_limit: limit.trim() === "" ? null : Number(limit),
          user_limit: userLimit.trim() === "" ? null : Number(userLimit),
          media_gb: mediaGb.trim() === "" ? null : Number(mediaGb),
          database_gb: databaseGb.trim() === "" ? null : Number(databaseGb),
          monthly_activities: activities.trim() === "" ? null : Number(activities),
        }),
      });
      setNameEn("");
      setNameKo("");
      setAmount("");
      setLimit("");
      setUserLimit("");
      setMediaGb("");
      setDatabaseGb("");
      setActivities("");
    });

  const setActive = (plan: Plan, active: boolean) =>
    run(() =>
      api(`/billing/plans/${plan.id}`, {
        method: "PATCH",
        body: JSON.stringify({
          name_en: plan.name_en,
          name_ko: plan.name_ko,
          instance_limit: plan.instance_limit,
          active,
        }),
      })
    );

  return (
    <Card className="gap-0 overflow-hidden p-0">
      <CardHeader className="px-4 py-3">
        <CardTitle className="text-base">Plans</CardTitle>
        <CardDescription className="text-xs">
          A plan's price cannot change: add a new plan and retire the old one. Subscribers keep
          the plan they are on until they change it.
        </CardDescription>
      </CardHeader>
      {error && (
        <Alert variant="destructive" className="mx-4 mb-3 w-auto">
          <CircleAlert />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead className="text-right">Monthly</TableHead>
            <TableHead className="text-right">Instances</TableHead>
            <TableHead>Shown limits</TableHead>
            <TableHead className="text-right">Subscribers</TableHead>
            <TableHead>Offered</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {plans.map((plan) => (
            <TableRow key={plan.id}>
              <TableCell>{plan.name_en} · {plan.name_ko}</TableCell>
              <TableCell className="text-right tabular-nums">{won(plan.amount)}</TableCell>
              <TableCell className="text-right tabular-nums">
                {plan.instance_limit ?? "Account's limit"}
              </TableCell>
              <TableCell className="text-muted-foreground text-xs">
                {[
                  plan.user_limit != null && `${plan.user_limit} users`,
                  plan.media_gb != null && `${plan.media_gb} GB media`,
                  plan.database_gb != null && `${plan.database_gb} GB database`,
                  plan.monthly_activities != null && `${plan.monthly_activities} activities`,
                ].filter(Boolean).join(" · ")}
              </TableCell>
              <TableCell className="text-right tabular-nums">{plan.subscribers}</TableCell>
              <TableCell>
                <Switch
                  aria-label={`Offer ${plan.name_en}`}
                  checked={plan.active}
                  disabled={busy}
                  onCheckedChange={(active) => void setActive(plan, active)}
                />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <form
        className="grid items-end gap-3 border-t p-4 md:grid-cols-4"
        onSubmit={(event) => {
          event.preventDefault();
          void create();
        }}
      >
        <div className="grid gap-1">
          <Label htmlFor="plan-name-en">English name</Label>
          <Input id="plan-name-en" required maxLength={60} value={nameEn} onChange={(event) => setNameEn(event.target.value)} />
        </div>
        <div className="grid gap-1">
          <Label htmlFor="plan-name-ko">Korean name</Label>
          <Input id="plan-name-ko" required maxLength={60} value={nameKo} onChange={(event) => setNameKo(event.target.value)} />
        </div>
        <div className="grid gap-1">
          <Label htmlFor="plan-amount">Won a month</Label>
          <Input id="plan-amount" required type="number" min={100} value={amount} onChange={(event) => setAmount(event.target.value)} />
        </div>
        <div className="grid gap-1">
          <Label htmlFor="plan-limit">Instance limit</Label>
          <Input id="plan-limit" type="number" min={0} placeholder="Account's" value={limit} onChange={(event) => setLimit(event.target.value)} />
        </div>
        <div className="grid gap-1">
          <Label htmlFor="plan-users">Users shown</Label>
          <Input id="plan-users" type="number" min={0} value={userLimit} onChange={(event) => setUserLimit(event.target.value)} />
        </div>
        <div className="grid gap-1">
          <Label htmlFor="plan-media">Media GB shown</Label>
          <Input id="plan-media" type="number" min={0} value={mediaGb} onChange={(event) => setMediaGb(event.target.value)} />
        </div>
        <div className="grid gap-1">
          <Label htmlFor="plan-database">Database GB shown</Label>
          <Input id="plan-database" type="number" min={0} value={databaseGb} onChange={(event) => setDatabaseGb(event.target.value)} />
        </div>
        <div className="grid gap-1">
          <Label htmlFor="plan-activities">Activities a month shown</Label>
          <Input id="plan-activities" type="number" min={0} value={activities} onChange={(event) => setActivities(event.target.value)} />
        </div>
        <Button type="submit" disabled={busy} className="md:col-span-4 md:justify-self-end">Add plan</Button>
      </form>
    </Card>
  );
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
    ["TossPayments mismatches", String(summary.mismatches), summary.mismatches > 0],
  ];

  return (
    <div className="grid gap-4">
      {!overview.enabled && (
        <Alert>
          <CreditCard />
          <AlertDescription>
            Billing is off: set TOSSPAYMENTS_CLIENT_KEY and TOSSPAYMENTS_SECRET_KEY on the
            control plane, and add a plan below, to start charging. Until then
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
      <Card className="grid grid-cols-2 gap-0 overflow-hidden p-0 md:grid-cols-4 xl:grid-cols-8">
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

      <Plans plans={overview.plans} onChanged={load} />

      <TossRecords overview={overview} onChanged={load} />

      <Card className="gap-0 overflow-hidden p-0">
        <CardHeader className="flex flex-col gap-3 px-4 py-3">
          <div>
            <CardTitle className="text-base">Subscribers</CardTitle>
            <CardDescription className="text-xs">
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
                              "The full amount goes back to the card. The subscription carries on and renews as before, and the account's instances stay online.",
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
