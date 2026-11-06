import { money, trialLength, won } from "@/format";
import { Fragment, useCallback, useEffect, useMemo, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  CircleAlert,
  CreditCard,
  ExternalLink,
  Pencil,
  RotateCcw,
  Trash2,
} from "lucide-react";
import { api, errorMessage } from "@/api";
import { goTo, useSubroute } from "@/router";
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
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
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
  provider: "toss" | "polar" | null;
  currency: string | null;
  card_company: string | null;
  card_number: string | null;
  has_card: boolean;
  current_period_end: string | null;
  next_attempt_at: string | null;
  cancel_at_period_end: boolean;
  failed_attempts: number;
  last_error: string | null;
  /** When the Polar free trial it is in ends. */
  trial_ends_at: string | null;
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
  polar_product_id: string | null;
  polar_amount: number | null;
  polar_currency: string | null;
  polar_trial_interval: string | null;
  polar_trial_interval_count: number | null;
  active: boolean;
  subscribers: number;
  /** Nothing refers to it, so it can be deleted rather than retired. */
  deletable: boolean;
};

type Overview = {
  enabled: boolean;
  toss: boolean;
  polar: boolean;
  plans: Plan[];
  /** Running subscriptions' monthly amounts, by currency. */
  revenue: { currency: string; amount: number; subscriptions: number }[];
  summary: {
    /** Paying; those in a free trial are `trialing`. */
    active: number;
    trialing: number;
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
  provider: "toss" | "polar";
  currency: string;
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
        : account.trial_ends_at
        ? { label: "trial", tone: "text-pending" }
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

/** A limit field's value: empty means none. */
const limitValue = (value: string) => (value.trim() === "" ? null : Number(value));
const limitText = (value: number | null) => (value == null ? "" : String(value));

/** Rename a plan and change its limits. Its price never changes. */
function EditPlanDialog(
  { plan, busy, onSave }: {
    plan: Plan;
    busy: boolean;
    onSave: (body: Record<string, unknown>) => Promise<boolean>;
  },
) {
  const [open, setOpen] = useState(false);
  const [nameEn, setNameEn] = useState(plan.name_en);
  const [nameKo, setNameKo] = useState(plan.name_ko);
  const [limit, setLimit] = useState(limitText(plan.instance_limit));
  const [users, setUsers] = useState(limitText(plan.user_limit));
  const [media, setMedia] = useState(limitText(plan.media_gb));
  const [database, setDatabase] = useState(limitText(plan.database_gb));
  const [activities, setActivities] = useState(limitText(plan.monthly_activities));
  const reset = () => {
    setNameEn(plan.name_en);
    setNameKo(plan.name_ko);
    setLimit(limitText(plan.instance_limit));
    setUsers(limitText(plan.user_limit));
    setMedia(limitText(plan.media_gb));
    setDatabase(limitText(plan.database_gb));
    setActivities(limitText(plan.monthly_activities));
  };
  const fields: [string, string, string, (value: string) => void, string?][] = [
    ["users", "Users shown", users, setUsers],
    ["media", "Media GB shown", media, setMedia],
    ["database", "Database GB shown", database, setDatabase],
    ["activities", "Activities a month shown", activities, setActivities],
  ];
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (next) reset();
        setOpen(next);
      }}
    >
      <DialogTrigger
        render={<Button size="icon-sm" variant="ghost" aria-label={`Edit ${plan.name_en}`} />}
      >
        <Pencil />
      </DialogTrigger>
      <DialogContent>
        <form
          className="grid gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            void onSave({
              name_en: nameEn,
              name_ko: nameKo,
              instance_limit: limitValue(limit),
              user_limit: limitValue(users),
              media_gb: limitValue(media),
              database_gb: limitValue(database),
              monthly_activities: limitValue(activities),
              active: plan.active,
            }).then((saved) => saved && setOpen(false));
          }}
        >
          <DialogHeader>
            <DialogTitle>Edit {plan.name_en}</DialogTitle>
            <DialogDescription>
              {won(plan.amount)} a month, which cannot change. Only the instance limit is
              enforced; the others are shown to customers. Leave one empty for no limit.
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="grid gap-1">
              <Label htmlFor={`edit-${plan.id}-en`}>English name</Label>
              <Input id={`edit-${plan.id}-en`} required maxLength={60} value={nameEn} onChange={(event) => setNameEn(event.target.value)} />
            </div>
            <div className="grid gap-1">
              <Label htmlFor={`edit-${plan.id}-ko`}>Korean name</Label>
              <Input id={`edit-${plan.id}-ko`} required maxLength={60} value={nameKo} onChange={(event) => setNameKo(event.target.value)} />
            </div>
            <div className="grid gap-1">
              <Label htmlFor={`edit-${plan.id}-limit`}>Instance limit</Label>
              <Input id={`edit-${plan.id}-limit`} type="number" min={0} placeholder="Account's" value={limit} onChange={(event) => setLimit(event.target.value)} />
            </div>
            {fields.map(([key, label, value, set]) => (
              <div key={key} className="grid gap-1">
                <Label htmlFor={`edit-${plan.id}-${key}`}>{label}</Label>
                <Input id={`edit-${plan.id}-${key}`} type="number" min={0} placeholder="No limit" value={value} onChange={(event) => set(event.target.value)} />
              </div>
            ))}
          </div>
          <DialogFooter>
            <Button type="submit" disabled={busy}>Save</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** Delete a plan nothing has used. One that was used can only be retired. */
function DeletePlanDialog(
  { plan, busy, onDelete }: { plan: Plan; busy: boolean; onDelete: () => Promise<boolean> },
) {
  const [open, setOpen] = useState(false);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger
        render={<Button size="icon-sm" variant="ghost" aria-label={`Delete ${plan.name_en}`} />}
      >
        <Trash2 />
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Delete {plan.name_en}?</DialogTitle>
          <DialogDescription>
            No subscription or payment has used it, so nothing is lost. This cannot be undone.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button
            variant="destructive"
            disabled={busy}
            onClick={() => void onDelete().then((deleted) => deleted && setOpen(false))}
          >
            Delete plan
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * The Polar product a plan is sold as, with its price, which follows Polar's.
 * It can be moved to another product, as from the sandbox to live products,
 * while no running Polar subscription is on the plan.
 */
function PolarProduct(
  { plan, busy, onLink, onRefresh }: {
    plan: Plan;
    busy: boolean;
    onLink: (productId: string) => Promise<boolean>;
    onRefresh: () => void;
  },
) {
  const linked = plan.polar_product_id && plan.polar_amount != null && plan.polar_currency;
  const [changing, setChanging] = useState(false);
  const [productId, setProductId] = useState("");
  if (linked && !changing) {
    return (
      <div className="flex items-center gap-1">
        <span className="tabular-nums" title={plan.polar_product_id ?? undefined}>
          {money(plan.polar_amount!, plan.polar_currency!)}
        </span>
        {trialLength(plan, false) && (
          <span className="text-muted-foreground text-xs">{trialLength(plan, false)}</span>
        )}
        <Button
          size="icon-xs"
          variant="ghost"
          aria-label={`Read ${plan.name_en}'s Polar price again`}
          title="Read the price from Polar again"
          disabled={busy}
          onClick={onRefresh}
        >
          <RotateCcw />
        </Button>
        <Button size="xs" variant="ghost" disabled={busy} onClick={() => setChanging(true)}>
          Change
        </Button>
      </div>
    );
  }
  return (
    <form
      className="flex gap-1"
      onSubmit={(event) => {
        event.preventDefault();
        void onLink(productId.trim()).then((done) => {
          if (done) {
            setChanging(false);
            setProductId("");
          }
        });
      }}
    >
      <Input
        aria-label={`Polar product for ${plan.name_en}`}
        placeholder="Product id"
        className="h-7 w-36 text-xs"
        maxLength={100}
        required
        value={productId}
        onChange={(event) => setProductId(event.target.value)}
      />
      <Button type="submit" size="xs" variant="outline" disabled={busy}>Link</Button>
      {changing && (
        <Button type="button" size="xs" variant="ghost" onClick={() => setChanging(false)}>
          Cancel
        </Button>
      )}
    </form>
  );
}

function Plans(
  { plans, polar, onChanged }: {
    plans: Plan[];
    polar: boolean;
    onChanged: () => Promise<void>;
  },
) {
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

  async function run(action: () => Promise<unknown>): Promise<boolean> {
    setBusy(true);
    setError("");
    try {
      await action();
      await onChanged();
      return true;
    } catch (failure) {
      setError(errorMessage(failure));
      return false;
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

  const savePlan = (plan: Plan, body: Record<string, unknown>) =>
    run(() => api(`/billing/plans/${plan.id}`, { method: "PATCH", body: JSON.stringify(body) }));

  const deletePlan = (plan: Plan) =>
    run(() => api(`/billing/plans/${plan.id}`, { method: "DELETE" }));

  const linkPolar = (plan: Plan, productId: string) =>
    run(() =>
      api(`/billing/plans/${plan.id}/polar`, {
        method: "POST",
        body: JSON.stringify({ product_id: productId }),
      })
    );

  const refreshPolar = (plan: Plan) =>
    run(() => api(`/billing/plans/${plan.id}/polar/refresh`, { method: "POST" }));

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
            {polar && <TableHead>International (Polar)</TableHead>}
            <TableHead className="text-right">Instances</TableHead>
            <TableHead>Shown limits</TableHead>
            <TableHead className="text-right">Subscribers</TableHead>
            <TableHead>Offered</TableHead>
            <TableHead className="w-0"><span className="sr-only">Actions</span></TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {plans.map((plan) => (
            <TableRow key={plan.id}>
              <TableCell>{plan.name_en} · {plan.name_ko}</TableCell>
              <TableCell className="text-right tabular-nums">{won(plan.amount)}</TableCell>
              {polar && (
                <TableCell>
                  <PolarProduct
                    plan={plan}
                    busy={busy}
                    onLink={(productId) => linkPolar(plan, productId)}
                    onRefresh={() => void refreshPolar(plan)}
                  />
                </TableCell>
              )}
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
              <TableCell>
                <div className="flex justify-end">
                  <EditPlanDialog plan={plan} busy={busy} onSave={(body) => savePlan(plan, body)} />
                  {plan.deletable && (
                    <DeletePlanDialog plan={plan} busy={busy} onDelete={() => deletePlan(plan)} />
                  )}
                </div>
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

/** The billing section's pages, each at /admin/billing/<id>. */
export const BILLING_PAGES = [
  { id: "", label: "Overview" },
  { id: "subscribers", label: "Subscribers" },
  { id: "plans", label: "Plans" },
  { id: "reconciliation", label: "Reconciliation" },
] as const;
export type BillingPage = (typeof BILLING_PAGES)[number]["id"];

export function billingPage(segments: string[]): BillingPage {
  return BILLING_PAGES.find((page) => page.id === (segments[0] ?? ""))?.id ?? "";
}

/** Links between the billing pages, for the phone layout's lack of a sidebar. */
function BillingPages({ page }: { page: BillingPage }) {
  return (
    <nav aria-label="Billing pages" className="flex flex-wrap gap-1 md:hidden">
      {BILLING_PAGES.map((item) => (
        <Button
          key={item.id}
          size="sm"
          variant={page === item.id ? "secondary" : "ghost"}
          aria-current={page === item.id ? "page" : undefined}
          onClick={() => goTo("billing", item.id ? [item.id] : [])}
        >
          {item.label}
        </Button>
      ))}
    </nav>
  );
}

type Tile = {
  label: string;
  value: string;
  /** What it counts, in a line. */
  hint: string;
  alarming?: boolean;
  /** Where to look into it. */
  open?: () => void;
};

function TileGroup({ title, description, tiles }: { title: string; description: string; tiles: Tile[] }) {
  return (
    <Card className="gap-0 overflow-hidden p-0">
      <CardHeader className="px-4 py-3">
        <CardTitle className="text-base">{title}</CardTitle>
        <CardDescription className="text-xs">{description}</CardDescription>
      </CardHeader>
      <Separator />
      <dl className="grid sm:grid-cols-2 lg:grid-cols-3">
        {tiles.map((tile) => {
          const body = (
            <>
              <dt className="text-muted-foreground text-xs">{tile.label}</dt>
              <dd
                className={cn(
                  "text-xl font-semibold tabular-nums",
                  tile.alarming && "text-destructive",
                )}
              >
                {tile.value}
              </dd>
              <dd className="text-muted-foreground text-xs">{tile.hint}</dd>
            </>
          );
          return tile.open
            ? (
              <button
                key={tile.label}
                type="button"
                className="hover:bg-accent/50 cursor-pointer border-b px-4 py-3 text-left sm:border-r"
                onClick={tile.open}
              >
                {body}
              </button>
            )
            : <div key={tile.label} className="border-b px-4 py-3 sm:border-r">{body}</div>;
        })}
      </dl>
    </Card>
  );
}

export function Billing() {
  const [overview, setOverview] = useState<Overview | null>(null);
  const [error, setError] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<string | null>(null);
  const [segments] = useSubroute("billing");
  const page = billingPage(segments);

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
  const subscribers = (show: Filter) => () => {
    setFilter(show);
    goTo("billing", ["subscribers"]);
  };
  const reconciliation = () => goTo("billing", ["reconciliation"]);
  const subscriptionTiles: Tile[] = [
    {
      label: "Monthly revenue",
      value: [
        won(summary.monthly_revenue),
        ...overview.revenue
          .filter((revenue) => revenue.currency !== "KRW")
          .map((revenue) => money(revenue.amount, revenue.currency)),
      ].join(" + "),
      hint: "What running subscriptions bring in each month, before fees. Trials and cancelling ones are left out.",
    },
    {
      label: "Active",
      value: String(summary.active),
      hint: "Paying and renewing.",
      open: subscribers("active"),
    },
    {
      label: "In a free trial",
      value: String(summary.trialing),
      hint: "Polar trials: nothing paid yet.",
      open: subscribers("active"),
    },
    {
      label: "Canceling",
      value: String(summary.canceling),
      hint: "Running until their paid period ends, then offline.",
      open: subscribers("canceling"),
    },
    {
      label: "Unbilled with instances",
      value: String(summary.unbilled),
      hint: "Running instances with no subscription or exemption.",
      open: subscribers("unbilled"),
    },
  ];
  const attentionTiles: Tile[] = [
    {
      label: "Past due",
      value: String(summary.past_due),
      hint: "A renewal was declined and is being retried.",
      alarming: summary.past_due > 0,
      open: subscribers("attention"),
    },
    {
      label: "Refunds open",
      value: String(summary.refunds_open),
      hint: "Asked for and not yet confirmed by the provider.",
      alarming: summary.refunds_open > 0,
      open: subscribers("attention"),
    },
    {
      label: "TossPayments mismatches",
      value: String(summary.mismatches),
      hint: "Where TossPayments' records and ours disagree.",
      alarming: summary.mismatches > 0,
      open: reconciliation,
    },
    {
      label: "Keys to delete",
      value: String(summary.keys_waiting),
      hint: "Old card keys TossPayments has not confirmed deleting.",
      alarming: summary.keys_waiting > 0,
      open: reconciliation,
    },
  ];

  return (
    <div className="grid gap-4">
      {!overview.enabled && (
        <Alert>
          <CreditCard />
          <AlertDescription>
            Billing is off: set TOSSPAYMENTS_CLIENT_KEY and TOSSPAYMENTS_SECRET_KEY, or
            POLAR_ACCESS_TOKEN and POLAR_WEBHOOK_SECRET, on the control plane, and add a plan
            under Plans, to start charging. Until then
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
      <BillingPages page={page} />

      {page === "" && (
        <>
          <TileGroup
            title="Subscriptions"
            description="Select a count to see those accounts."
            tiles={subscriptionTiles}
          />
          <TileGroup
            title="Needs attention"
            description="Anything above zero here is waiting on a provider or on you."
            tiles={attentionTiles}
          />
        </>
      )}

      {page === "plans" && <Plans plans={overview.plans} polar={overview.polar} onChanged={load} />}

      {page === "reconciliation" && <TossRecords overview={overview} onChanged={load} />}

      {page === "subscribers" && (
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
                              ? money(account.amount, account.currency ?? "KRW")
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
      )}
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
                    <TableCell className="text-right tabular-nums">
                      {money(payment.amount, payment.currency)}
                    </TableCell>
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
                            title: `Refund ${money(payment.amount, payment.currency)} to ${account.email}?`,
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
