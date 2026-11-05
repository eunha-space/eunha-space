import { won } from "@/format";
import { useEffect, useState } from "react";
import { CircleAlert, CreditCard, ExternalLink } from "lucide-react";
import { useLingui } from "@lingui/react";
import { loadTossPayments } from "@tosspayments/tosspayments-sdk";
import { api, type Billing, type Customer, errorMessage, type Plan } from "@/api";
import { cn } from "cn";
import { absoluteTime } from "@/format";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Spinner } from "@/components/console/status";
import { CONTACT_EMAIL, planLimits } from "./planLimits";
import { RefundPolicy } from "./RefundPolicy";

/** Where TossPayments' billing window sends the customer back, either way. */
const RETURN_PATH = "/account/billing";

/**
 * A billing window's `authKey` can be exchanged once. React may run an effect
 * twice, so the exchange is remembered outside the component.
 */
let returnHandled = false;

function day(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleDateString();
}

export function SubscriptionCard(
  { customer, onChanged }: { customer: Customer; onChanged: () => Promise<void> },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const [billing, setBilling] = useState<Billing | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [confirmingCancel, setConfirmingCancel] = useState(false);
  const [refunding, setRefunding] = useState<Billing["payments"][number] | null>(null);
  const [selectedPlan, setSelectedPlan] = useState<string | null>(null);
  const [switching, setSwitching] = useState<Plan | null>(null);

  useEffect(() => {
    let active = true;
    async function load() {
      const query = new URLSearchParams(location.search);
      if (location.pathname === RETURN_PATH && !returnHandled) {
        returnHandled = true;
        // Leave the one-time key out of the address bar and history.
        history.replaceState(null, "", "/account");
        const authKey = query.get("authKey");
        const customerKey = query.get("customerKey");
        if (authKey && customerKey) {
          setBusy(true);
          try {
            const next = await api<Billing>("/customer/billing/authorization", {
              method: "POST",
              body: JSON.stringify({ auth_key: authKey, customer_key: customerKey }),
            });
            if (active) setBilling(next);
            await onChanged();
          } catch (e) {
            if (active) setError(t(errorMessage(e)));
          } finally {
            if (active) setBusy(false);
          }
        } else if (query.get("code") && query.get("code") !== "PAY_PROCESS_CANCELED") {
          // TossPayments' own message, written for the customer.
          setError(query.get("message") || t("The card could not be registered."));
        }
      }
      const next = await api<Billing>("/customer/billing");
      if (active) setBilling(next);
    }
    load().catch((e) => active && setError(t(errorMessage(e))));
    return () => {
      active = false;
    };
  }, []);

  if (!billing || !billing.enabled) return null;

  async function openBillingWindow() {
    setError("");
    setBusy(true);
    try {
      const checkout = await api<{ client_key: string; customer_key: string }>(
        "/customer/billing/checkout",
        { method: "POST", body: JSON.stringify({ plan_id: choosing ? chosenPlan?.id : undefined }) },
      );
      const toss = await loadTossPayments(checkout.client_key);
      await toss.payment({ customerKey: checkout.customer_key }).requestBillingAuth({
        method: "CARD",
        successUrl: location.origin + RETURN_PATH,
        failUrl: location.origin + RETURN_PATH,
        customerEmail: customer.email,
      });
    } catch (e) {
      // Closing the window is not a failure worth reporting.
      const code = (e as { code?: string } | null)?.code;
      if (code !== "USER_CANCEL") setError(t(errorMessage(e)));
    } finally {
      setBusy(false);
    }
  }

  async function refund(orderId: string) {
    setError("");
    setBusy(true);
    try {
      setBilling(
        await api<Billing>(
          `/customer/billing/payments/${encodeURIComponent(orderId)}/refund`,
          { method: "POST" },
        ),
      );
      await onChanged();
    } catch (e) {
      setError(t(errorMessage(e)));
    } finally {
      setBusy(false);
      setRefunding(null);
    }
  }

  async function changePlan(plan: Plan) {
    setError("");
    setBusy(true);
    try {
      setBilling(
        await api<Billing>("/customer/billing/plan", {
          method: "POST",
          body: JSON.stringify({ plan_id: plan.id }),
        }),
      );
      await onChanged();
    } catch (e) {
      setError(t(errorMessage(e)));
      // A declined upgrade leaves a failed payment in the history.
      setBilling(await api<Billing>("/customer/billing").catch(() => billing));
    } finally {
      setBusy(false);
      setSwitching(null);
    }
  }

  async function change(path: "/cancel" | "/resume") {
    setError("");
    setBusy(true);
    try {
      setBilling(await api<Billing>(`/customer/billing${path}`, { method: "POST" }));
      await onChanged();
    } catch (e) {
      setError(t(errorMessage(e)));
    } finally {
      setBusy(false);
      setConfirmingCancel(false);
    }
  }

  const subscription = billing.subscription;
  const status = subscription?.status;
  const inGrace = subscription?.refund_grace_until != null;
  const subscribed = (status === "active" || status === "past_due") && !inGrace;
  const pending = billing.payments.some((payment) => payment.status === "pending");
  const selectable = billing.plans.filter((plan) => plan.active);
  const choosing = (!subscribed || subscription?.refund_grace_until != null) && !pending;
  const chosenPlan = selectable.find((plan) => plan.id === selectedPlan) ?? selectable[0] ?? null;
  const planName = (plan: Plan) => (i18n.locale === "ko" ? plan.name_ko : plan.name_en);
  const currentPlan = billing.plans.find((plan) => plan.id === subscription?.plan_id) ?? null;
  const nextPlan = billing.plans.find((plan) => plan.id === subscription?.pending_plan_id) ?? null;
  const amount = subscription && subscribed && !choosing
    ? subscription.amount
    : chosenPlan?.amount ?? null;
  const limitLabel = (plan: Plan) => planLimits(plan, i18n.locale === "ko").join(" · ");

  let description: string;
  if (inGrace && !pending) {
    description = t(
      "Your payment was refunded. Your instances keep running until the date below, then go offline with their data kept. Subscribe again to keep them running.",
    );
  } else if (status === "active" && subscription?.cancel_at_period_end) {
    description = t(
      "Your subscription is cancelled. Your instances keep running until the end of the paid period, then go offline with their data kept.",
    );
  } else if (status === "active") {
    description = t("Your card is charged automatically each month.");
  } else if (status === "past_due") {
    description = t(
      "Your last payment was declined. We'll try again, and your instances keep running meanwhile. Use another card to pay now.",
    );
  } else if (pending) {
    description = t("Your payment is being confirmed.");
  } else if (status === "canceled" || status === "lapsed") {
    description = t(
      "Your subscription has ended, so your instances are offline with their data kept. Subscribe again to serve them again.",
    );
  } else {
    description = t("Subscribe to create and run instances.");
  }

  return (
    <Card className={customer.subscription_required ? "border-primary/40" : undefined}>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          {t("Subscription")}
          {status === "active" && !subscription?.cancel_at_period_end && (
            <Badge>{t("Active")}</Badge>
          )}
          {status === "active" && subscription?.cancel_at_period_end && (
            <Badge variant="secondary">{t("Cancelled")}</Badge>
          )}
          {status === "past_due" && (
            <Badge variant="destructive">{t("Payment declined")}</Badge>
          )}
        </CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent className="grid gap-5">
        <dl className="grid grid-cols-2 gap-4 text-sm">
          {subscribed && currentPlan && (
            <div className="grid gap-1">
              <dt className="text-muted-foreground text-xs">{t("Plan")}</dt>
              <dd className="font-medium">{planName(currentPlan)}</dd>
            </div>
          )}
          {status === "active" && nextPlan && !subscription?.cancel_at_period_end && (
            <div className="grid gap-1">
              <dt className="text-muted-foreground text-xs">{t("From your next renewal")}</dt>
              <dd className="font-medium">{planName(nextPlan)} · {won(nextPlan.amount)}</dd>
            </div>
          )}
          {amount != null && (
            <div className="grid gap-1">
              <dt className="text-muted-foreground text-xs">{t("Monthly price")}</dt>
              <dd className="font-medium">{won(amount)}</dd>
            </div>
          )}
          {subscribed && subscription?.has_card && (
            <div className="grid gap-1">
              <dt className="text-muted-foreground text-xs">{t("Card")}</dt>
              <dd className="font-medium">
                {[subscription.card_company, subscription.card_number]
                  .filter(Boolean)
                  .join(" ")}
              </dd>
            </div>
          )}
          {status === "active" && subscription?.current_period_end && (
            <div className="grid gap-1">
              <dt className="text-muted-foreground text-xs">
                {subscription.cancel_at_period_end ? t("Ends on") : t("Renews on")}
              </dt>
              <dd className="font-medium">
                {day(subscription.refund_grace_until ?? subscription.current_period_end)}
              </dd>
            </div>
          )}
          {status === "past_due" && subscription?.next_attempt_at && (
            <div className="grid gap-1">
              <dt className="text-muted-foreground text-xs">{t("Next attempt")}</dt>
              <dd className="font-medium">{absoluteTime(subscription.next_attempt_at)}</dd>
            </div>
          )}
        </dl>
        {subscription?.last_error &&
          (status === "past_due" || status === "incomplete" || inGrace) &&
          !pending && (
          <Alert variant="destructive">
            <CircleAlert />
            <AlertDescription>{t(subscription.last_error)}</AlertDescription>
          </Alert>
        )}
        {error && (
          <Alert variant="destructive">
            <CircleAlert />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}
        {(choosing || (status === "active" && !subscription?.cancel_at_period_end)) &&
          selectable.length > 1 && (
          <div className="grid gap-2">
            <h3 className="text-sm font-medium">{choosing ? t("Choose a plan") : t("Change plan")}</h3>
            <div className="grid gap-2 sm:grid-cols-2">
              {selectable.map((plan) => {
                const isCurrent = !choosing && plan.id === subscription?.plan_id;
                const selected = choosing ? plan.id === chosenPlan?.id : isCurrent;
                return (
                  <button
                    key={plan.id}
                    type="button"
                    aria-pressed={selected}
                    disabled={busy || (isCurrent && !nextPlan)}
                    onClick={() => (choosing ? setSelectedPlan(plan.id) : setSwitching(plan))}
                    className={cn(
                      "grid gap-0.5 rounded-lg border p-3 text-left text-sm transition-colors disabled:cursor-default",
                      selected ? "border-primary bg-primary/5" : "hover:bg-muted",
                    )}
                  >
                    <span className="font-medium">
                      {planName(plan)}
                      {isCurrent ? ` · ${t("Current plan")}` : ""}
                    </span>
                    <span className="tabular-nums">{won(plan.amount)}{t(" a month")}</span>
                    {limitLabel(plan) && (
                      <span className="text-muted-foreground text-xs">{limitLabel(plan)}</span>
                    )}
                  </button>
                );
              })}
              <a
                href={`mailto:${CONTACT_EMAIL}`}
                className="hover:bg-muted grid gap-0.5 rounded-lg border p-3 text-left text-sm transition-colors"
              >
                <span className="font-medium">{t("Custom")}</span>
                <span>{t("Contact us")}</span>
                <span className="text-muted-foreground text-xs">
                  {t("For bigger instances, we'll work out a plan with you.")}
                </span>
              </a>
            </div>
          </div>
        )}
        <div className="flex flex-wrap gap-2">
          {!subscribed && !pending && (
            <Button disabled={busy || !chosenPlan} onClick={() => void openBillingWindow()}>
              {busy ? <Spinner label={t("Please wait…")} /> : <CreditCard />}
              {t("Subscribe with a card")}
            </Button>
          )}
          {subscribed && (
            <Button
              variant={status === "past_due" ? "default" : "outline"}
              disabled={busy}
              onClick={() => void openBillingWindow()}
            >
              <CreditCard />
              {status === "past_due" ? t("Use another card") : t("Change card")}
            </Button>
          )}
          {subscribed && !subscription?.cancel_at_period_end && (
            <Button
              variant="ghost"
              disabled={busy}
              onClick={() => setConfirmingCancel(true)}
            >
              {t("Cancel subscription")}
            </Button>
          )}
          {subscribed && status === "active" && subscription?.cancel_at_period_end &&
            subscription.has_card && (
            <Button disabled={busy} onClick={() => void change("/resume")}>
              {t("Keep my subscription")}
            </Button>
          )}
        </div>
        {billing.payments.length > 0 && (
          <div className="grid gap-2">
            <h3 className="text-sm font-medium">{t("Payments")}</h3>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t("Period")}</TableHead>
                  <TableHead className="text-right">{t("Amount")}</TableHead>
                  <TableHead>{t("Status")}</TableHead>
                  <TableHead />
                </TableRow>
              </TableHeader>
              <TableBody>
                {billing.payments.map((payment) => (
                  <TableRow key={payment.order_id}>
                    <TableCell>
                      {day(payment.period_start)} – {day(payment.period_end)}
                    </TableCell>
                    <TableCell className="text-right tabular-nums">
                      {won(payment.amount)}
                    </TableCell>
                    <TableCell>
                      {payment.status === "refunded"
                        ? t("Refunded")
                        : payment.refund_pending
                        ? t("Refund pending")
                        : payment.status === "paid" && payment.receipt_url
                        ? (
                          <a
                            href={payment.receipt_url}
                            target="_blank"
                            rel="noreferrer"
                            className="inline-flex items-center gap-1 underline-offset-4 hover:underline"
                          >
                            {t("Paid")} <ExternalLink className="size-3" />
                          </a>
                        )
                        : payment.status === "paid"
                        ? t("Paid")
                        : payment.status === "pending"
                        ? t("Confirming")
                        : (
                          <span
                            className="text-destructive"
                            title={payment.failure_message ?? undefined}
                          >
                            {t("Declined")}
                          </span>
                        )}
                      {payment.refund_error && !payment.refund_pending && (
                        <p className="text-destructive text-xs">
                          {t("Refund refused")}: {t(payment.refund_error)}
                        </p>
                      )}
                    </TableCell>
                    <TableCell className="text-right">
                      {payment.refundable && (
                        <Button
                          size="sm"
                          variant="ghost"
                          disabled={busy}
                          onClick={() => setRefunding(payment)}
                        >
                          {t("Refund")}
                        </Button>
                      )}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
        <AlertDialog
          open={refunding !== null}
          onOpenChange={(open) => !open && setRefunding(null)}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                {refunding ? `${t("Refund this payment?")} ${won(refunding.amount)}` : ""}
              </AlertDialogTitle>
              <AlertDialogDescription>
                {refunding?.kind === "upgrade" ? t("The full amount goes back to your card, you return to the plan you had, and your subscription is cancelled. It keeps running until the end of the paid period.") : t(
                  "The full amount goes back to your card and your subscription is cancelled. Refunding the period you are in leaves your instances running for four more days, then they go offline. Their data is kept, and subscribing again serves them again.",
                )}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{t("Keep my subscription")}</AlertDialogCancel>
              <AlertDialogAction
                variant="destructive"
                onClick={() => refunding && void refund(refunding.order_id)}
              >
                {t("Refund")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
        <AlertDialog open={confirmingCancel} onOpenChange={setConfirmingCancel}>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>{t("Cancel your subscription?")}</AlertDialogTitle>
              <AlertDialogDescription>
                {status === "past_due"
                  ? t(
                    "Your paid period is already over, so your instances go offline now. Their data is kept, and subscribing again serves them again.",
                  )
                  : t(
                    "Your instances keep running until the end of the paid period, then go offline. Their data is kept, and subscribing again serves them again.",
                  )}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{t("Keep my subscription")}</AlertDialogCancel>
              <AlertDialogAction
                variant="destructive"
                onClick={() => void change("/cancel")}
              >
                {t("Cancel subscription")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
        <AlertDialog
          open={switching !== null}
          onOpenChange={(open) => !open && setSwitching(null)}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                {switching ? `${t("Change to this plan?")} ${planName(switching)}` : ""}
              </AlertDialogTitle>
              <AlertDialogDescription>
                {switching && switching.id === subscription?.plan_id
                  ? t("You stay on your current plan, and the change waiting for your next renewal is dropped.")
                  : switching?.switch_charge
                  ? `${t("It starts now, and we charge your card the difference for the rest of this period:")} ${won(switching.switch_charge)}. ${t("Your renewal date stays the same.")}`
                  : switching && switching.amount > (subscription?.amount ?? 0)
                  ? t("It starts now at no charge, because so little of this period is left.")
                  : t("It starts at your next renewal. Nothing changes until then, and you can change your mind.")}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{t("Keep this plan")}</AlertDialogCancel>
              <AlertDialogAction onClick={() => switching && void changePlan(switching)}>
                {t("Change plan")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
        <RefundPolicy />
      </CardContent>
    </Card>
  );
}
