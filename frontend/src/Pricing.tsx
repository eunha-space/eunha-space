import { useEffect, useState } from "react";
import { useLingui } from "@lingui/react";
import { api, type Customer, type PublicPlan } from "@/api";
import { Button } from "@/components/ui/button";
import { won } from "@/format";
import { CONTACT_EMAIL, planLimits } from "@/planLimits";

const TERMS = [
  "Every plan is charged monthly.",
  "A payment can be refunded in full for 7 days after it is made, and not after that.",
  "Cancel any time and keep the period you already paid for.",
  "If a subscription ends, your instances go offline and their data is kept. Subscribing again brings them back.",
  "A deleted instance can be restored for seven days.",
];

/** The plans on offer, at `/pricing`, for anyone. */
export function Pricing({ customer }: { customer: Customer | null }) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const [plans, setPlans] = useState<PublicPlan[]>([]);
  useEffect(() => {
    // Without prices the page still says how to ask and what the terms are.
    api<PublicPlan[]>("/customer/plans").then(setPlans).catch(() => {});
  }, []);
  const korean = i18n.locale === "ko";
  // A customer subscribes and changes plans on their account page.
  const action = !customer
    ? { href: "/signup", label: t("Create account") }
    : { href: "/account", label: t(customer.subscription_required ? "Subscribe" : "Change plan") };
  return (
    <main className="mx-auto w-full max-w-5xl flex-1 break-keep px-4 md:px-8">
      <section className="py-12 md:py-16">
        <h1 className="text-4xl font-semibold tracking-tight text-balance md:text-5xl">
          {t("Pricing")}
        </h1>
        <p className="text-muted-foreground mt-4 max-w-xl text-lg text-pretty">
          {t("Every plan includes hosting, automatic updates and an address of your own.")}
        </p>
        <ul className="mt-10 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {plans.map((plan) => (
            <li key={plan.name_en} className="flex flex-col rounded-lg border p-5">
              <h2 className="font-medium">{korean ? plan.name_ko : plan.name_en}</h2>
              <p className="mt-2">
                <span className="text-3xl font-semibold tracking-tight tabular-nums">
                  {won(plan.amount)}
                </span>{" "}
                <span className="text-muted-foreground text-sm">{t("/ month")}</span>
              </p>
              <ul className="text-muted-foreground mt-4 mb-6 grid gap-1 text-sm">
                {planLimits(plan, korean).map((limit) => <li key={limit}>{limit}</li>)}
              </ul>
              <Button variant="outline" className="mt-auto" render={<a href={action.href} />}>
                {action.label}
              </Button>
            </li>
          ))}
          <li className="flex flex-col rounded-lg border p-5">
            <h2 className="font-medium">{t("Custom")}</h2>
            <p className="text-muted-foreground mt-2 mb-6 text-sm">
              {t("For bigger instances, we'll work out a plan with you.")}
            </p>
            <Button
              variant="outline"
              className="mt-auto"
              render={<a href={`mailto:${CONTACT_EMAIL}`} />}
            >
              {t("Contact us")}
            </Button>
          </li>
        </ul>
      </section>
      <section className="border-t py-12 md:py-16">
        <ul className="text-muted-foreground grid max-w-2xl list-disc gap-2 pl-5">
          {TERMS.map((term) => <li key={term}>{t(term)}</li>)}
        </ul>
      </section>
    </main>
  );
}
