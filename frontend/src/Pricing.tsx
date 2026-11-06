import { useEffect, useState } from "react";
import { cn } from "cn";
import { useLingui } from "@lingui/react";
import { api, type Customer, type PublicPlan } from "@/api";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Page, PageHeader } from "@/components/customer/page";
import { money, trialLength, won } from "@/format";
import { CONTACT_EMAIL, planLimits } from "@/planLimits";

const TERMS = [
  "Every plan is charged monthly.",
  "A payment can be refunded in full for 7 days after it is made, and not after that.",
  "With an international card, only your first payment can be refunded from your account page; write to us about any other.",
  "Cancel any time and keep the period you already paid for.",
  "If a subscription ends, your instances go offline and their data is kept. Subscribing again brings them back.",
  "A deleted instance can be restored for seven days.",
];

/** The plans on offer, at `/pricing`, for anyone. */
/** `customer` is undefined until the session is known, null when signed out. */
export function Pricing({ customer }: { customer: Customer | null | undefined }) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const [plans, setPlans] = useState<PublicPlan[]>([]);
  const [loaded, setLoaded] = useState(false);
  useEffect(() => {
    // Without prices the page still says how to ask and what the terms are.
    api<PublicPlan[]>("/customer/plans")
      .then(setPlans)
      .catch(() => {})
      .finally(() => setLoaded(true));
  }, []);
  const korean = i18n.locale === "ko";
  // A customer subscribes and changes plans on their account page.
  const action = !customer
    ? { href: "/signup", label: t("Create account") }
    : { href: "/account", label: t(customer.subscription_required ? "Subscribe" : "Change plan") };
  return (
    <Page>
      <PageHeader
        title={t("Pricing")}
        description={t("Every plan includes hosting, automatic updates and an address of your own.")}
      />
      <ul className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {!loaded && [0, 1, 2].map((key) => (
          <li key={key}><Skeleton className="h-72 rounded-xl" /></li>
        ))}
        {plans.map((plan) => (
          <li key={plan.name_en} className="grid">
            <PlanCard title={korean ? plan.name_ko : plan.name_en}>
              <p>
                <span className="text-3xl font-semibold tracking-tight tabular-nums">
                  {won(plan.amount)}
                </span>{" "}
                <span className="text-muted-foreground text-sm">{t("/ month")}</span>
                {plan.polar_amount != null && plan.polar_currency && (
                  <span className="text-muted-foreground block text-sm tabular-nums">
                    {t("International cards:")} {money(plan.polar_amount, plan.polar_currency)}{" "}
                    {t("/ month")}
                    {trialLength(plan, korean) && ` · ${trialLength(plan, korean)}`}
                  </span>
                )}
              </p>
              <ul className="text-muted-foreground grid gap-1 text-sm">
                {planLimits(plan, korean).map((limit) => <li key={limit}>{limit}</li>)}
              </ul>
              <Button
                // Held in place, unseen, until it is known which one to offer.
                className={cn("mt-auto", customer === undefined && "invisible")}
                render={<a href={action.href} />}
              >
                {action.label}
              </Button>
            </PlanCard>
          </li>
        ))}
        <li className="grid">
          <PlanCard title={t("Custom")}>
            <p className="text-muted-foreground text-sm">
              {t("For bigger instances, we'll work out a plan with you.")}
            </p>
            <Button
              variant="outline"
              className="mt-auto"
              render={<a href={`mailto:${CONTACT_EMAIL}`} />}
            >
              {t("Contact us")}
            </Button>
          </PlanCard>
        </li>
      </ul>
      <section className="grid gap-3 border-t pt-6">
        <h2 className="text-xl font-semibold tracking-tight">{t("Billing terms")}</h2>
        <ul className="text-muted-foreground grid list-disc gap-2 pl-5">
          {TERMS.map((term) => <li key={term}>{t(term)}</li>)}
        </ul>
        <p>
          <a href="/refund-policy" className="text-sm underline underline-offset-4">
            {t("Refund policy")}
          </a>
        </p>
      </section>
    </Page>
  );
}

function PlanCard({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle><h2>{title}</h2></CardTitle>
      </CardHeader>
      <CardContent className="flex flex-1 flex-col gap-4">{children}</CardContent>
    </Card>
  );
}
