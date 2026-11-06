import { type ReactNode, useEffect, useState } from "react";
import { useLingui } from "@lingui/react";
import { ArrowLeft, Check, CircleAlert, CircleCheck, Clock } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Spinner } from "@/components/console/status";
import { cn } from "cn";

/**
 * The pieces every customer page is built from, so that a page's edges, title,
 * sections and states look the same wherever the reader goes next.
 */

/** The site's one column: the header, every page and the footer share its edges. */
/**
 * The one column every customer page keeps to, header and footer included, so
 * that nothing moves sideways between pages. The operators' console is apart.
 */
export const siteColumn = "mx-auto w-full max-w-3xl px-4 md:px-8";

/** A page below the header, in the site's column. */
export function Page({ children }: { children: ReactNode }) {
  return (
    <main className={cn(siteColumn, "flex-1 py-8 md:py-12")}>
      <div className="grid gap-6">{children}</div>
    </main>
  );
}

/**
 * The top of a page. `back` is for a page below another; the pages the header
 * links to have none, since the header is already there to leave them by.
 */
export function PageHeader(
  { back, title, badge, description, action }: {
    back?: { href: string; label: string };
    title: ReactNode;
    badge?: ReactNode;
    description?: ReactNode;
    action?: ReactNode;
  },
) {
  return (
    <div className="grid gap-3">
      {back && <BackLink {...back} />}
      <div className="flex flex-wrap items-start justify-between gap-x-6 gap-y-3">
        <div className="grid min-w-0 gap-2">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <h1 className="min-w-0 text-3xl font-semibold tracking-tight wrap-anywhere">
              {title}
            </h1>
            {badge}
          </div>
          {description && <p className="text-muted-foreground text-pretty">{description}</p>}
        </div>
        {action}
      </div>
    </div>
  );
}

export function BackLink({ href, label }: { href: string; label: string }) {
  return (
    <div>
      <Button variant="ghost" className="-ml-2.5" render={<a href={href} />}>
        <ArrowLeft /> {label}
      </Button>
    </div>
  );
}

/** One topic on a page. */
export function SectionCard(
  { title, badge, description, highlight = false, children }: {
    title: ReactNode;
    badge?: ReactNode;
    description?: ReactNode;
    /** For the section the reader has to act on first. */
    highlight?: boolean;
    children?: ReactNode;
  },
) {
  return (
    <Card className={highlight ? "ring-primary/40" : undefined}>
      <CardHeader>
        <CardTitle>
          <h2 className="flex flex-wrap items-center gap-2">
            {title}
            {badge}
          </h2>
        </CardTitle>
        {description && <CardDescription>{description}</CardDescription>}
      </CardHeader>
      {children && <CardContent className="grid gap-4">{children}</CardContent>}
    </Card>
  );
}

/** A titled part of a public page, with its title beside the text on a wide screen. */
export function ProseSection(
  { id, title, children }: { id?: string; title: string; children: ReactNode },
) {
  return (
    <section
      id={id}
      className="grid scroll-mt-4 gap-6 border-t py-12 md:grid-cols-[12rem_1fr] md:gap-10 md:py-16"
    >
      <h2 className="text-xl font-semibold tracking-tight">{title}</h2>
      <div>{children}</div>
    </section>
  );
}

/** Waiting for a page or a section's first answer. */
/**
 * A wait shorter than this reads as instant, and a spinner that flashes up and
 * away only makes it look slower; so the spinner waits this long to appear.
 */
const SPINNER_DELAY_MS = 300;

export function Loading({ inline = false }: { inline?: boolean }) {
  const { i18n } = useLingui();
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const timer = window.setTimeout(() => setVisible(true), SPINNER_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, []);
  // The space is kept either way, so the page does not move when it appears.
  return (
    <div className={cn("grid place-items-center", inline ? "py-4" : "min-h-64")}>
      {visible && (
        <Spinner
          label={i18n._("Loading…")}
          className="text-muted-foreground animate-in fade-in duration-300 motion-reduce:animate-none"
        />
      )}
    </div>
  );
}

/** A failure. Server messages are catalog keys too, so they localize here. */
export function ErrorAlert({ message }: { message: string }) {
  const { i18n } = useLingui();
  // Two messages carry a value, so they are looked up by their pattern.
  const reference = /^Request could not be completed\. Reference: (\S+)$/.exec(message);
  const failed = /^Request failed \((\d+)\)$/.exec(message);
  const text = reference
    ? i18n._("Request could not be completed. Reference: {id}", { id: reference[1] })
    : failed
      ? i18n._("Request failed ({status})", { status: failed[1] })
      : i18n._(message);
  return (
    <Alert variant="destructive">
      <CircleAlert />
      <AlertDescription>{text}</AlertDescription>
    </Alert>
  );
}

/** Something the reader asked for has been done. */
export function SuccessAlert({ children }: { children: ReactNode }) {
  return (
    <Alert role="status">
      <CircleCheck className="text-success" />
      <AlertDescription>{children}</AlertDescription>
    </Alert>
  );
}

/** Something the reader has to do before a page can offer them more. */
export function ActionAlert(
  { icon, children, href, action }: {
    icon: ReactNode;
    children: ReactNode;
    href: string;
    action: string;
  },
) {
  // Laid out like the cards beside it, highlighted like a section to act on.
  return (
    <div
      role="status"
      className="bg-card ring-primary/40 flex flex-wrap items-center gap-x-3 gap-y-3 rounded-xl px-4 py-3 text-sm ring-1 [&>svg]:size-4 [&>svg]:shrink-0"
    >
      {icon}
      <p className="min-w-0 flex-1 basis-48">{children}</p>
      <Button size="sm" render={<a href={href} />}>{action}</Button>
    </div>
  );
}

/** A labelled value, such as a plan's name or the date a card is charged. */
export function Detail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid content-start gap-1">
      <dt className="text-muted-foreground text-xs">{label}</dt>
      <dd className="min-w-0 text-sm font-medium wrap-anywhere">{children}</dd>
    </div>
  );
}

const BADGE_TONE = {
  ok: { variant: "secondary", className: undefined },
  waiting: { variant: "outline", className: "text-pending" },
  problem: { variant: "destructive", className: undefined },
  neutral: { variant: "outline", className: "text-muted-foreground" },
} as const;

/** The state of something: fine, under way, in trouble, or simply over. */
export function StateBadge(
  { tone, children }: { tone: keyof typeof BADGE_TONE; children: ReactNode },
) {
  return <Badge {...BADGE_TONE[tone]}>{children}</Badge>;
}

export type StepStatus = "pending" | "running" | "succeeded" | "failed";

/** The stages of a long job, for a customer waiting on it. */
export function ProgressSteps(
  { steps }: { steps: { label: string; status: StepStatus; note?: string }[] },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const STATUS_TEXT: Record<StepStatus, string> = {
    succeeded: "Done",
    running: "In progress",
    failed: "Needs attention",
    pending: "Waiting",
  };
  return (
    <ol className="grid gap-4">
      {steps.map(({ label, status, note }) => (
        <li key={label} className="flex items-center gap-3">
          {/* Keyed by status, so each change is drawn rather than swapped. */}
          <span
            key={status}
            className="animate-in fade-in zoom-in-75 flex shrink-0 duration-300 motion-reduce:animate-none"
          >
            {status === "succeeded"
              ? <Check className="text-success size-5" />
              : status === "running"
              ? <Spinner label={t("In progress")} className="text-pending size-5" />
              : status === "failed"
              ? <CircleAlert className="text-destructive size-5" />
              : <Clock className="text-muted-foreground size-5" />}
          </span>
          <span className="flex-1 text-sm font-medium">{t(label)}</span>
          <span
            key={`${status}-text`}
            className="text-muted-foreground animate-in fade-in text-xs duration-300 motion-reduce:animate-none"
          >
            {note ?? t(STATUS_TEXT[status])}
          </span>
        </li>
      ))}
    </ol>
  );
}

export type AddressKind = "provided" | "custom";

/** Whether an instance's address is one of ours or the customer's own domain. */
export function AddressKindToggle(
  { value, onChange }: { value: AddressKind; onChange: (kind: AddressKind) => void },
) {
  const { i18n } = useLingui();
  return (
    <ToggleGroup
      variant="outline"
      size="sm"
      // Base UI keeps single-select state as an array and clears it when the
      // pressed item is pressed again; keep the choice.
      value={[value]}
      onValueChange={(next) => onChange((next[0] as AddressKind | undefined) ?? value)}
    >
      <ToggleGroupItem value="provided">{i18n._("eunha space address")}</ToggleGroupItem>
      <ToggleGroupItem value="custom">{i18n._("My own domain")}</ToggleGroupItem>
    </ToggleGroup>
  );
}
