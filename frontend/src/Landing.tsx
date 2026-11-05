import { useEffect, useState } from "react";
import { useLingui } from "@lingui/react";
import { api, type PublicPlan } from "@/api";
import { Button } from "@/components/ui/button";
import { Mark, ThemeToggle } from "@/components/console/brand";
import { won } from "@/format";
import { LanguageToggle } from "@/i18n";
import { RefundPolicy } from "./RefundPolicy";
import { planLimits } from "@/planLimits";

const EXPLANATIONS = [
  {
    term: "The fediverse",
    definition:
      "A network of independently run social servers that talk to each other over an open protocol, ActivityPub. Mastodon is the best known of them. It works like email: whichever server your account is on, you can reach people on the others.",
  },
  {
    term: "Eunha",
    definition:
      "Free, open-source server software that reimplements Mastodon in Rust. It tracks each Mastodon release and works with Mastodon's apps.",
  },
  {
    term: "eunha space",
    definition:
      "Hosting for Eunha. We run the server and keep it up to date, so you can spend your time on your community instead.",
  },
];

const FEATURES = [
  {
    title: "An address of your own",
    body:
      "Start with a name under eunha.space, or use a domain you own by pointing a CNAME at us. You can move to another domain later, and your followers are told where you went.",
  },
  {
    title: "Your server, your rules",
    body:
      "You are the server's owner, with its tools for members, moderation and settings. Registrations start closed, so you decide who joins.",
  },
  {
    title: "The Mastodon apps you know",
    body:
      "Eunha speaks Mastodon's API, so you can keep using the Mastodon apps and clients you like.",
  },
  {
    title: "Nothing to install",
    body:
      "New Eunha releases and their database migrations are rolled out for you. Media storage is ready from the start.",
  },
  {
    title: "Email that works",
    body:
      "On an eunha.space address, your server's email is already set up. On your own domain, add your SMTP provider on the instance page.",
  },
  {
    title: "Free to leave",
    body:
      "Members can export their data and move their accounts, followers included, to another server. Eunha itself is open source under the AGPL.",
  },
];

const STEPS = [
  {
    title: "Create an account",
    body: "All you need is an email address. Follow the link we send you.",
  },
  {
    title: "Subscribe",
    body: "Choose a monthly plan and pay by card.",
  },
  {
    title: "Create your instance",
    body:
      "Pick an address and an admin username. Setup runs by itself, and with your own domain it starts once the domain validates.",
  },
  {
    title: "Sign in to your server",
    body:
      "Create your admin password on the instance page, then sign in at your new address.",
  },
];

const TERMS = [
  "Every plan is charged monthly.",
  "A payment can be refunded in full for 7 days after it is made, and not after that.",
  "Cancel any time and keep the period you already paid for.",
  "If a subscription ends, your instances go offline and their data is kept. Subscribing again brings them back.",
  "A deleted instance can be restored for seven days.",
];

/** What a signed-out visitor sees at `/`. */
export function Landing() {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const [plans, setPlans] = useState<PublicPlan[]>([]);
  useEffect(() => {
    // Without prices the rest of the page still explains the service.
    api<PublicPlan[]>("/customer/plans").then(setPlans).catch(() => {});
  }, []);
  const korean = i18n.locale === "ko";
  return (
    <main data-landing className="mx-auto w-full max-w-5xl flex-1 break-keep px-4 md:px-8">
      <header className="flex items-center justify-between gap-3 py-5">
        <a href="/" className="flex min-w-0 items-center gap-2 font-semibold">
          <Mark className="size-9 shrink-0" />
          {t("eunha space")}
        </a>
        <div className="flex shrink-0 gap-2">
          <LanguageToggle />
          <ThemeToggle />
          <Button variant="outline" render={<a href="/signin" />}>
            {t("Sign in")}
          </Button>
        </div>
      </header>

      <section className="grid items-center gap-10 py-12 md:grid-cols-[3fr_2fr] md:py-20">
        <div>
          <h1 className="text-4xl font-semibold tracking-tight text-balance md:text-6xl">
            {t("Your own Mastodon-compatible server, run for you.")}
          </h1>
          <p className="text-muted-foreground mt-6 max-w-xl text-lg text-pretty">
            {t(
              "eunha space hosts Eunha, a Mastodon-compatible fediverse server. Choose an address and you have a server of your own, where you can follow and be followed by people on Mastodon and across the fediverse.",
            )}
          </p>
          <div className="mt-8 flex flex-wrap gap-3">
            <Button size="lg" render={<a href="/signup" />}>
              {t("Create account")}
            </Button>
            <Button size="lg" variant="outline" render={<a href="#how-it-works" />}>
              {t("How it works")}
            </Button>
          </div>
        </div>
        <Constellation label={t("Your server")} />
      </section>

      <Section title={t("What is this?")}>
        <dl className="grid gap-6">
          {EXPLANATIONS.map(({ term, definition }) => (
            <div key={term}>
              <dt className="font-medium">{t(term)}</dt>
              <dd className="text-muted-foreground mt-1">{t(definition)}</dd>
            </div>
          ))}
        </dl>
      </Section>

      <Section title={t("What you get")}>
        <div className="grid gap-x-10 gap-y-6 sm:grid-cols-2">
          {FEATURES.map(({ title, body }) => (
            <div key={title}>
              <h3 className="font-medium">{t(title)}</h3>
              <p className="text-muted-foreground mt-1">{t(body)}</p>
            </div>
          ))}
        </div>
      </Section>

      <Section id="how-it-works" title={t("How it works")}>
        <ol className="grid gap-6">
          {STEPS.map(({ title, body }, index) => (
            <li key={title} className="grid grid-cols-[2rem_1fr]">
              <span className="text-primary font-mono font-medium" aria-hidden="true">
                {index + 1}
              </span>
              <div>
                <h3 className="font-medium">{t(title)}</h3>
                <p className="text-muted-foreground mt-1">{t(body)}</p>
              </div>
            </li>
          ))}
        </ol>
      </Section>

      <Section title={t("Pricing")}>
        {plans.length > 0 && (
          <ul className="mb-6 divide-y border-y">
            {plans.map((plan) => (
              <li
                key={plan.name_en}
                className="flex flex-wrap items-baseline justify-between gap-x-6 gap-y-1 py-3"
              >
                <div>
                  <h3 className="font-medium">
                    {korean ? plan.name_ko : plan.name_en}
                  </h3>
                  {planLimits(plan, korean).length > 0 && (
                    <p className="text-muted-foreground text-sm">
                      {planLimits(plan, korean).join(" · ")}
                    </p>
                  )}
                </div>
                <p>
                  <span className="font-medium">{won(plan.amount)}</span>{" "}
                  <span className="text-muted-foreground text-sm">{t("/ month")}</span>
                </p>
              </li>
            ))}
          </ul>
        )}
        <ul className="text-muted-foreground grid list-disc gap-2 pl-5">
          {TERMS.map((term) => <li key={term}>{t(term)}</li>)}
        </ul>
        <div className="mt-8">
          <RefundPolicy />
        </div>
      </Section>

      <section className="border-t py-12 md:py-16">
        <h2 className="text-2xl font-semibold tracking-tight text-balance md:text-3xl">
          {t("Start your own server.")}
        </h2>
        <div className="mt-6 flex flex-wrap items-center gap-x-5 gap-y-3">
          <Button size="lg" render={<a href="/signup" />}>
            {t("Create account")}
          </Button>
          <a
            href="https://eunha.social"
            className="text-muted-foreground hover:text-foreground text-sm underline underline-offset-4"
          >
            {t("About Eunha")}
          </a>
          <a
            href="https://github.com/eunha-space/eunha"
            className="text-muted-foreground hover:text-foreground text-sm underline underline-offset-4"
          >
            {t("Source code")}
          </a>
        </div>
      </section>
    </main>
  );
}

function Section(
  { id, title, children }: {
    id?: string;
    title: string;
    children: React.ReactNode;
  },
) {
  return (
    <section
      id={id}
      className="grid scroll-mt-4 gap-6 border-t py-12 md:grid-cols-[12rem_1fr] md:gap-10 md:py-16"
    >
      <h2 className="text-xl font-semibold tracking-tight">{title}</h2>
      <div className="max-w-2xl">{children}</div>
    </section>
  );
}

const OTHER_SERVERS = [
  { x: 92, y: 62, name: "mastodon.social" },
  { x: 346, y: 84, name: "hackers.pub" },
  { x: 356, y: 288, name: "misskey.io" },
  { x: 82, y: 276, name: "" },
  { x: 214, y: 32, name: "" },
  { x: 232, y: 330, name: "" },
];
const LINKS_BETWEEN_OTHERS = [[0, 4], [4, 1], [1, 2], [2, 5], [5, 3], [3, 0]];
const CENTER = { x: 214, y: 176 };

/** Your server among others, drawn as the galaxy eunha (은하) is named for. */
function Constellation({ label }: { label: string }) {
  return (
    <svg
      viewBox="0 0 430 360"
      className="mx-auto w-full max-w-sm md:max-w-none"
      aria-hidden="true"
    >
      <g className="stroke-border" strokeWidth="1.5">
        {LINKS_BETWEEN_OTHERS.map(([from, to]) => (
          <line
            key={`${from}-${to}`}
            x1={OTHER_SERVERS[from].x}
            y1={OTHER_SERVERS[from].y}
            x2={OTHER_SERVERS[to].x}
            y2={OTHER_SERVERS[to].y}
          />
        ))}
      </g>
      <g className="stroke-primary/60" strokeWidth="1.5">
        {OTHER_SERVERS.map(({ x, y }) => (
          <line key={`${x}-${y}`} x1={CENTER.x} y1={CENTER.y} x2={x} y2={y} />
        ))}
      </g>
      {OTHER_SERVERS.map(({ x, y, name }) => (
        <g key={`${x}-${y}`}>
          <circle cx={x} cy={y} r="6" className="fill-pending" />
          {name && (
            <text
              x={x}
              y={y < CENTER.y ? y - 14 : y + 24}
              textAnchor="middle"
              fontSize="14"
              className="fill-muted-foreground"
            >
              {name}
            </text>
          )}
        </g>
      ))}
      {/* Paints the background over the links behind the labels. */}
      <rect
        x={CENTER.x - 86}
        y={CENTER.y - 30}
        width="172"
        height="84"
        className="fill-background"
      />
      <circle cx={CENTER.x} cy={CENTER.y - 8} r="13" className="fill-primary" />
      <text
        x={CENTER.x}
        y={CENTER.y + 26}
        textAnchor="middle"
        fontSize="15"
        fontWeight="600"
        className="fill-foreground"
      >
        {label}
      </text>
      <text
        x={CENTER.x}
        y={CENTER.y + 45}
        textAnchor="middle"
        fontSize="14"
        className="fill-primary font-mono"
      >
        garden.eunha.space
      </text>
    </svg>
  );
}
