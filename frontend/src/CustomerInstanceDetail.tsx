import { MemberUsage } from "@/components/customer/usage";
import { useEffect, useState } from "react";
import { Copy, ExternalLink, KeyRound, RotateCcw, Trash2 } from "lucide-react";
import { useLingui } from "@lingui/react";
import {
  api,
  type CustomerDomainSetup,
  type CustomerProvisioningStep,
  type CustomerTenantDetail,
  errorMessage,
  instanceAddress,
  isOffline,
} from "@/api";
import { absoluteTime } from "@/format";
import { navigate } from "@/navigation";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/console/status";
import {
  BackLink,
  Detail,
  ErrorAlert,
  Loading,
  Page,
  PageHeader,
  ProgressSteps,
  SectionCard,
  StateBadge,
} from "@/components/customer/page";
import { InstanceStatusBadge } from "@/components/customer/instance";
import { CustomerSmtp } from "@/CustomerSmtp";
import { CustomerDomainChange } from "@/CustomerDomainChange";
import { CustomerGettingStarted } from "@/CustomerGettingStarted";

// Provisioning is tracked step by step, but a customer only needs the shape of
// the wait, not the mechanics — and the routing step in particular reads as
// homework on an instance whose address we hand out ourselves.
const STEP_GROUPS: {
  label: string;
  steps: CustomerProvisioningStep["step"][];
}[] = [
  { label: "Preparing your instance", steps: ["database", "migrate", "render"] },
  { label: "Starting your instance", steps: ["reload", "health", "routing"] },
  { label: "Creating your admin account", steps: ["owner"] },
];

function groupStatus(
  steps: CustomerProvisioningStep[],
): CustomerProvisioningStep["status"] {
  if (steps.some((step) => step.status === "failed")) return "failed";
  if (steps.every((step) => step.status === "succeeded")) return "succeeded";
  // Part of a group being done is progress, even while nothing is running.
  if (steps.some((step) => step.status !== "pending")) return "running";
  return "pending";
}

function stepGroups(steps: CustomerProvisioningStep[]) {
  return STEP_GROUPS.map((group) => ({
    label: group.label,
    steps: steps.filter((step) => group.steps.includes(step.step)),
    // An instance provisioned without an owner has no owner step at all.
  })).filter((group) => group.steps.length > 0);
}

// Setup can finish faster than anyone can read it, and a page that jumps from
// "preparing" to a finished instance reads as though nothing happened. So the
// steps are shown in order, each held long enough to be seen: a change is
// drawn in about 300ms, the usual length of an interface transition, and the
// next one waits until the last has been on screen for STEP_DWELL_MS. This
// only ever lags what has really happened; a step is never shown done early.
const STEP_DWELL_MS = 900;
// How long the finished setup stays before the instance's own cards replace it.
const READY_HOLD_MS = 1500;

type StepStatus = CustomerProvisioningStep["status"];

function prefersReducedMotion() {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

/**
 * The statuses to draw for `actual`, catching up one change at a time.
 * `replay` starts every step as waiting, for a page opened right after the
 * instance was created; otherwise what is already done is shown as done.
 */
function usePacedSteps(actual: StepStatus[] | null, replay: boolean) {
  const [shown, setShown] = useState<StepStatus[] | null>(null);
  const [settled, setSettled] = useState(false);
  const key = actual?.join() ?? "";

  useEffect(() => {
    if (!actual) return;
    if (shown === null || shown.length !== actual.length) {
      const first = replay
        ? actual.map((status): StepStatus => status === "failed" ? "failed" : "pending")
        : actual;
      setShown(first);
      if (!replay && first.every((status) => status === "succeeded")) setSettled(true);
      return;
    }
    const index = shown.findIndex((status, i) => status !== actual[i]);
    if (index === -1) return;
    // Something going wrong is news, not progress: it is shown at once.
    const failed = actual[index] === "failed";
    const next = shown.slice();
    // A step that went straight from waiting to done still shows it ran.
    next[index] = shown[index] === "pending" && actual[index] === "succeeded"
      ? "running"
      : actual[index];
    const timer = window.setTimeout(
      () => setShown(next),
      failed || prefersReducedMotion() ? 0 : STEP_DWELL_MS,
    );
    return () => window.clearTimeout(timer);
  }, [key, shown, replay]);

  const done = shown !== null &&
    shown.every((status) => status === "succeeded");
  useEffect(() => {
    if (!done || settled) return;
    const timer = window.setTimeout(
      () => setSettled(true),
      prefersReducedMotion() ? 0 : READY_HOLD_MS,
    );
    return () => window.clearTimeout(timer);
  }, [done, settled]);

  return { shown, done, settled };
}

export function CustomerInstanceDetail({ id }: { id: string }) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  // Read once: a reload of the page later shows the instance as it is.
  const [replay] = useState(() => {
    const params = new URLSearchParams(location.search);
    if (!params.has("created")) return false;
    history.replaceState(history.state, "", location.pathname);
    return true;
  });
  const [detail, setDetail] = useState<CustomerTenantDetail | null>(null);
  const [error, setError] = useState("");
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    let active = true;
    const load = async () => {
      try {
        const next = await api<CustomerTenantDetail>(`/customer/tenants/${id}`);
        if (active) {
          setDetail(next);
          setError("");
        }
      } catch (failure) {
        if (active) setError(errorMessage(failure));
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), 3000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [id, revision]);

  const groups = detail ? stepGroups(detail.provisioning_steps) : null;
  const paced = usePacedSteps(
    groups?.map((group) => groupStatus(group.steps)) ?? null,
    replay,
  );

  const back = { href: "/instances", label: t("Your instances") };
  if (!detail && !error) {
    return (
      <Page>
        <Loading />
      </Page>
    );
  }
  if (!detail) {
    return (
      <Page>
        <BackLink {...back} />
        <ErrorAlert message={error} />
      </Page>
    );
  }

  const customDomains = detail.domains.filter((domain) =>
    domain.kind === "custom"
  );
  const ready = detail.tenant.lifecycle_state === "ready";
  // With setup done the admin account, which is what the customer came for,
  // takes the top of the page.
  // Done once it has also been seen to finish; until then the setup card stays.
  const setupDone = ready && paced.settled &&
    detail.provisioning_steps.every((step) => step.status === "succeeded");
  const accountReady = ready && detail.tenant.owner_ready && paced.settled;
  // Nothing provisions until a pending custom domain resolves, so that card
  // holds the only useful thing to read while it is outstanding.
  const waitingForDomain = customDomains.some((domain) =>
    domain.status === "pending"
  );
  // A deleted instance is not being served, so setup, domain and admin details
  // would describe something that is not there.
  const offline = isOffline(detail.tenant);
  return (
    <Page>
      <PageHeader
        back={back}
        title={instanceAddress(detail.tenant)}
        // Ready is announced once the setup card has shown it, not before.
        badge={
          <InstanceStatusBadge
            tenant={ready && !paced.settled
              ? { lifecycle_state: "provisioning" }
              : detail.tenant}
          />
        }
        action={!offline && ready && paced.settled && (
          <Button
            variant="outline"
            render={
              <a
                href={`https://${instanceAddress(detail.tenant)}`}
                target="_blank"
                rel="noreferrer"
              />
            }
          >
            <ExternalLink /> {t("Open instance")}
          </Button>
        )}
      />
        {!offline && (
          // Swapping the setup for the finished instance's cards fades them in,
          // so the change of page reads as one.
          <div
            key={setupDone ? "ready" : "setup"}
            className="animate-in fade-in slide-in-from-bottom-2 grid gap-6 duration-300 motion-reduce:animate-none"
          >
          {(() => {
          // Once every step has succeeded there is nothing left to follow.
          const setup = setupDone
            ? null
            : (
              <InstanceSetup
                key="setup"
                labels={(groups ?? []).map((group) => group.label)}
                statuses={paced.shown ?? []}
                finished={paced.done}
              />
            );
          const domain = customDomains.length > 0
            ? (
              <CustomDomainSetup
                key="domain"
                tenant={detail.tenant}
                domains={customDomains}
              />
            )
            : null;
          // Handles on another domain fix both domains together.
          const move = !detail.tenant.handle_domain &&
              (setupDone || detail.domain_change)
            ? (
              <CustomerDomainChange
                key="move"
                tenant={detail.tenant}
                change={detail.domain_change}
                onChanged={() => setRevision((value) => value + 1)}
              />
            )
            : null;
          const account = accountReady && detail.tenant.owner_username
            ? (
              <AdminAccount
                key="account"
                tenantId={detail.tenant.id}
                domain={detail.tenant.domain}
                username={detail.tenant.owner_username}
                email={detail.tenant.owner_email ?? ""}
                passwordIssued={detail.tenant.owner_password_issued_at !== null}
              />
            )
            : null;
          // Whatever the customer needs most goes first: their account once
          // there is one, their DNS while it holds everything up, otherwise
          // the progress of the setup they are waiting on.
          // A move under way is what the customer is watching.
          if (detail.domain_change?.status === "running") {
            return [move, domain, account, setup];
          }
          // Next to the account, what to do with it before inviting anyone.
          const guide = account
            ? <CustomerGettingStarted key="guide" domain={detail.tenant.domain} />
            : null;
          return setupDone
            ? [account, guide, move, domain, setup]
            : waitingForDomain
            ? [domain, setup, account]
            : [setup, domain, account];
          })()}
          </div>
        )}
        {ready &&
          detail.domains.some((domain) =>
            domain.kind === "custom" && domain.status === "active" && domain.is_primary
          ) && <CustomerSmtp key={id} tenantId={id} />}
        {ready && <MemberUsage tenantId={id} />}
        <DeleteInstance
          tenant={detail.tenant}
          onChanged={() => setRevision((value) => value + 1)}
        />
    </Page>
  );
}

function InstanceSetup(
  { labels, statuses, finished }: {
    labels: string[];
    statuses: StepStatus[];
    finished: boolean;
  },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  return (
    <SectionCard
      title={t("Instance setup")}
      description={finished
        ? (
          <span className="text-success animate-in fade-in duration-300">
            {t("Your instance is ready to use.")}
          </span>
        )
        : t("This page updates automatically while we set things up.")}
    >
      <ProgressSteps
        steps={labels.map((label, index) => ({
          label,
          status: statuses[index] ?? "pending",
        }))}
      />
    </SectionCard>
  );
}

type IssuedPassword = { username: string; email: string; password: string };

function AdminAccount(
  { tenantId, domain, username, email, passwordIssued }: {
    tenantId: string;
    domain: string;
    username: string;
    email: string;
    passwordIssued: boolean;
  },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const [issued, setIssued] = useState<IssuedPassword | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState("");

  async function issue() {
    setConfirming(false);
    setBusy(true);
    setError("");
    setIssued(null);
    setCopied(false);
    try {
      // The password lives only in this component's state; the server keeps no
      // copy, so leaving the page is the end of it.
      setIssued(
        await api<IssuedPassword>(`/customer/tenants/${tenantId}/owner/password`, {
          method: "POST",
        }),
      );
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  }

  async function copy() {
    if (!issued) return;
    try {
      await navigator.clipboard.writeText(issued.password);
      setCopied(true);
    } catch {
      setError(t("The password could not be copied. Select it and copy it instead."));
    }
  }

  const loginUrl = `https://${domain}/account/login`;
  const button = (
    <Button
      variant={passwordIssued ? "outline" : "default"}
      disabled={busy}
      onClick={passwordIssued ? undefined : () => void issue()}
    >
      {busy ? <Spinner label={t("In progress")} /> : <KeyRound />}
      {passwordIssued ? t("Reset password") : t("Create password")}
    </Button>
  );

  return (
    <SectionCard
      title={t("Admin account")}
      description={t("Sign in to your instance as its owner to manage it.")}
    >
        <dl className="grid gap-4 sm:grid-cols-2">
          <Detail label={t("Sign-in email")}>{email}</Detail>
          <Detail label={t("Username")}>@{username}</Detail>
          <Detail label={t("Sign-in page")}>
            <a
              className="underline-offset-4 hover:underline"
              href={loginUrl}
              target="_blank"
              rel="noreferrer"
            >
              {loginUrl}
            </a>
          </Detail>
        </dl>

        {issued && (
          <div className="bg-muted/60 grid gap-3 rounded-md p-3 text-sm">
            <div className="grid gap-1">
              <span className="text-muted-foreground text-xs">
                {t("New password")}
              </span>
              <div className="flex flex-wrap items-center gap-2">
                <code className="flex-1 break-all select-all">
                  {issued.password}
                </code>
                <Button variant="ghost" size="sm" onClick={() => void copy()}>
                  <Copy />
                  {copied ? t("Copied") : t("Copy")}
                </Button>
              </div>
            </div>
            <p className="text-muted-foreground text-xs">
              {t("Save this password now. It is not stored and will not be shown again.")}
            </p>
          </div>
        )}

        {error && <ErrorAlert message={error} />}

        <div>
          {passwordIssued && !busy
            ? (
              <AlertDialog open={confirming} onOpenChange={setConfirming}>
                <AlertDialogTrigger render={button} />
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>{t("Reset the admin password?")}</AlertDialogTitle>
                    <AlertDialogDescription>
                      {t(
                        "A new password is generated, and the admin account is signed out everywhere it is signed in.",
                      )}
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>{t("Cancel")}</AlertDialogCancel>
                    <AlertDialogAction onClick={() => void issue()}>
                      {t("Reset password")}
                    </AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            )
            : button}
        </div>
    </SectionCard>
  );
}

function DeleteInstance(
  { tenant, onChanged }: {
    tenant: CustomerTenantDetail["tenant"];
    onChanged: () => void;
  },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [confirming, setConfirming] = useState<"delete" | "purge" | null>(null);
  const [confirmation, setConfirmation] = useState("");
  const address = instanceAddress(tenant);

  async function act(path: string, body?: object) {
    setConfirming(null);
    setBusy(true);
    setError("");
    try {
      await api(`/customer/tenants/${tenant.id}${path}`, {
        method: "POST",
        body: body ? JSON.stringify(body) : undefined,
      });
      if (path === "/purge") {
        // A purged instance is gone from the customer's list.
        navigate("/instances");
        return;
      }
      onChanged();
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
      setConfirmation("");
    }
  }

  const state = tenant.lifecycle_state;
  const retained = state === "retained";
  let description: string;
  if (retained && tenant.abandoned_custom_domain) {
    description = `${tenant.abandoned_custom_domain} · ${
      t(
        "This instance was deleted because its custom domain wasn't connected within 7 days, so it was never set up. Restore it to try the domain again.",
      )
    }`;
  } else if (retained && tenant.billing_suspended) {
    description = t(
      "This instance is offline because your subscription ended. Its data is kept, and subscribing again serves it again.",
    );
  } else if (retained) {
    description = t(
      "This instance is offline. Its data is kept until the date below, then permanently deleted unless you restore it.",
    );
  } else if (state === "deprovisioning") {
    description = t("Taking this instance offline. Its data is kept for 7 days.");
  } else {
    description = t(
      "Deleting takes this instance offline right away. Its data is kept for 7 days so you can restore it, then permanently deleted.",
    );
  }

  return (
    <SectionCard
      title={retained ? t("Deleted instance") : t("Delete instance")}
      description={description}
    >
        {(retained || state === "deprovisioning") && tenant.purge_scheduled_for && (
          <dl>
            <Detail label={t("Permanently deleted on")}>
              {absoluteTime(tenant.purge_scheduled_for)}
            </Detail>
          </dl>
        )}
        {error && <ErrorAlert message={error} />}
        <div className="flex flex-wrap gap-2">
          {retained && (
            <>
              <Button disabled={busy} onClick={() => void act("/restore")}>
                {busy ? <Spinner label={t("In progress")} /> : <RotateCcw />}
                {t("Restore instance")}
              </Button>
              <Button
                variant="destructive"
                disabled={busy}
                onClick={() => setConfirming("purge")}
              >
                <Trash2 /> {t("Delete permanently now")}
              </Button>
            </>
          )}
          {state === "deprovisioning" && (
            <Spinner label={t("In progress")} className="text-pending size-5" />
          )}
          {(state === "ready" || state === "failed" || state === "provisioning") && (
            <Button
              variant="destructive"
              disabled={busy}
              onClick={() => setConfirming("delete")}
            >
              <Trash2 /> {t("Delete instance")}
            </Button>
          )}
        </div>
        <AlertDialog
          open={confirming !== null}
          onOpenChange={(open) => {
            if (!open) {
              setConfirming(null);
              setConfirmation("");
            }
          }}
        >
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                {confirming === "purge"
                  ? t("Delete this instance permanently?")
                  : t("Delete this instance?")}
              </AlertDialogTitle>
              <AlertDialogDescription>
                {confirming === "purge"
                  ? t(
                    "Its accounts, posts, media and settings are destroyed now and can't be restored.",
                  )
                  : t(
                    "It goes offline right away. Its data is permanently deleted after 7 days unless you restore it.",
                  )}
              </AlertDialogDescription>
            </AlertDialogHeader>
            {confirming === "purge" && (
              <div className="grid gap-2">
                <span className="text-muted-foreground text-xs">
                  {t("Type the instance's address to confirm")} · {address}
                </span>
                <Input
                  autoComplete="off"
                  value={confirmation}
                  onChange={(event) => setConfirmation(event.target.value)}
                />
              </div>
            )}
            <AlertDialogFooter>
              <AlertDialogCancel>{t("Cancel")}</AlertDialogCancel>
              <AlertDialogAction
                variant="destructive"
                disabled={confirming === "purge" &&
                  confirmation.trim().toLowerCase() !== address.toLowerCase()}
                onClick={() =>
                  void (confirming === "purge"
                    ? act("/purge", { confirm_domain: confirmation })
                    : act("/deletion"))}
              >
                {confirming === "purge"
                  ? t("Delete permanently")
                  : t("Delete instance")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
    </SectionCard>
  );
}

function CustomDomainSetup(
  { tenant, domains }: {
    tenant: CustomerTenantDetail["tenant"];
    domains: CustomerDomainSetup[];
  },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  return (
    <SectionCard
      title={t("Custom domain setup")}
      description={t("Keep each domain's DNS record in place for as long as it is connected.")}
    >
        {domains.map((domain) => (
          <section key={domain.hostname} className="grid gap-3">
            <div className="flex items-center justify-between gap-3">
              <strong className="break-all">{domain.hostname}</strong>
              <StateBadge tone={domain.status === "active" ? "ok" : "waiting"}>
                {domain.status === "active"
                  ? t("Connected")
                  : domain.status === "removing"
                  ? t("Removing")
                  : t("Waiting for DNS")}
              </StateBadge>
            </div>
            {domain.status === "active" && (
              <DnsRecord type="CNAME" name={domain.hostname} value={domain.cname_target} />
            )}
            {domain.status === "removing" && (
              <p className="text-muted-foreground text-sm">
                {t("This domain is being released.")}
              </p>
            )}
            {domain.status === "pending" && (
              <>
                <p className="text-muted-foreground text-sm">
                  {t("Add this CNAME record at your DNS provider.")}
                </p>
                <DnsRecord
                  type="CNAME"
                  name={domain.hostname}
                  value={domain.cname_target}
                />
                {domain.ownership_name && domain.ownership_value && (
                  <>
                    <p className="text-muted-foreground text-sm">
                      {t("If requested, also add this verification record.")}
                    </p>
                    <DnsRecord
                      type="TXT"
                      name={domain.ownership_name}
                      value={domain.ownership_value}
                    />
                  </>
                )}
                {domain.check_problem && (
                  <p className="text-pending text-sm">
                    {domain.check_problem === "dns"
                      ? t("Its DNS doesn't point to customers.eunha.space yet. We'll keep checking.")
                      : t("We couldn't finish checking this domain yet. We'll keep trying.")}
                  </p>
                )}
                {domain.setup_deadline && (
                  <dl>
                    <Detail label={t("If it isn't connected by then, this instance is deleted")}>
                      {absoluteTime(domain.setup_deadline)}
                    </Detail>
                  </dl>
                )}
              </>
            )}
          </section>
        ))}
        {tenant.handle_domain && (
          <HandleDomainSetup
            handleDomain={tenant.handle_domain}
            webDomain={tenant.domain}
            verified={tenant.handle_domain_verified}
          />
        )}
    </SectionCard>
  );
}

/** Handles on another domain: that domain's server sends WebFinger here. */
function HandleDomainSetup(
  { handleDomain, webDomain, verified }: {
    handleDomain: string;
    webDomain: string;
    verified: boolean;
  },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  return (
    <section className="grid gap-3">
      <div className="flex items-center justify-between gap-3">
        <strong className="break-all">{handleDomain}</strong>
        <StateBadge tone={verified ? "ok" : "waiting"}>
          {verified ? t("Redirecting") : t("Waiting for the redirect")}
        </StateBadge>
      </div>
      <p className="text-muted-foreground text-sm">
        {verified
          ? i18n._(
            "Handles on this instance end in @{domain}. Keep this redirect in place: other servers find accounts through it.",
            { domain: handleDomain },
          )
          : i18n._(
            "Handles on this instance end in @{domain}. Have the web server for {domain} redirect /.well-known/webfinger here; with nginx:",
            { domain: handleDomain },
          )}
      </p>
      <pre className="bg-muted/60 overflow-x-auto rounded-md p-3 text-xs"><code>{`location = /.well-known/webfinger {
    return 301 https://${webDomain}$request_uri;
}`}</code></pre>
    </section>
  );
}

function DnsRecord(
  { type, name, value }: { type: string; name: string; value: string },
) {
  const { i18n } = useLingui();
  return (
    <dl className="bg-muted/60 grid gap-3 rounded-md p-3 sm:grid-cols-[auto_1fr_1fr]">
      <Detail label={i18n._("Type")}><code>{type}</code></Detail>
      <Detail label={i18n._("Name")}><code>{name}</code></Detail>
      <Detail label={i18n._("Value")}><code>{value}</code></Detail>
    </dl>
  );
}
