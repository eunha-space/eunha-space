import { useEffect, useState } from "react";
import {
  ArrowLeft,
  Check,
  CircleAlert,
  Clock,
  Copy,
  KeyRound,
  RotateCcw,
  Trash2,
} from "lucide-react";
import { useLingui } from "@lingui/react";
import {
  api,
  type CustomerDomainSetup,
  type CustomerProvisioningStep,
  type CustomerTenantDetail,
  errorMessage,
  instanceAddress,
  instanceStatus,
} from "@/api";
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
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Spinner } from "@/components/console/status";

const STEP_LABELS: Record<CustomerProvisioningStep["step"], string> = {
  database: "Preparing storage",
  migrate: "Installing the app",
  render: "Saving your settings",
  reload: "Starting your instance",
  health: "Checking availability",
  routing: "Connecting domains",
  owner: "Creating your admin account",
};

export function CustomerInstanceDetail({ id }: { id: string }) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
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

  if (!detail && !error) {
    return (
      <div className="grid min-h-64 place-items-center">
        <Spinner label={t("Loading…")} />
      </div>
    );
  }
  if (!detail) {
    return (
      <Alert variant="destructive">
        <CircleAlert />
        <AlertDescription>{error}</AlertDescription>
      </Alert>
    );
  }

  const customDomains = detail.domains.filter((domain) =>
    domain.kind === "custom"
  );
  const ready = detail.tenant.lifecycle_state === "ready";
  const status = instanceStatus(detail.tenant);
  // A deleted instance is not being served, so setup, domain and admin details
  // would describe something that is not there.
  const offline = detail.tenant.lifecycle_state === "deprovisioning" ||
    detail.tenant.lifecycle_state === "retained" ||
    detail.tenant.lifecycle_state === "purged";
  return (
    <div className="grid gap-6">
      <div>
        <Button variant="ghost" className="mb-3 -ml-3" render={<a href="/" />}>
          <ArrowLeft /> {t("All instances")}
        </Button>
        <div className="flex flex-wrap items-center gap-3">
          <h1 className="text-2xl font-semibold tracking-tight">
            {detail.tenant.title}
          </h1>
          <Badge
            variant={status === "Ready" ? "secondary" : "outline"}
            className={status === "Ready" ? undefined : "text-pending"}
          >
            {t(status)}
          </Badge>
        </div>
        <a
          className="text-muted-foreground hover:text-foreground text-sm underline-offset-4 hover:underline"
          href={`https://${instanceAddress(detail.tenant)}`}
          target="_blank"
          rel="noreferrer"
        >
          {instanceAddress(detail.tenant)}
        </a>
      </div>

      <div className="grid gap-6">
        {!offline && (
          <>
            {detail.tenant.owner_username && (
              <AdminAccount
                tenantId={detail.tenant.id}
                domain={detail.tenant.domain}
                username={detail.tenant.owner_username}
                email={detail.tenant.owner_email ?? ""}
                ready={ready && detail.tenant.owner_ready}
                passwordIssued={detail.tenant.owner_password_issued_at !== null}
                waitingForDomain={customDomains.some((domain) =>
                  domain.status === "pending"
                )}
              />
            )}
            <CustomDomainSetup tenant={detail.tenant} domains={customDomains} />

            <Card>
              <CardHeader>
                <CardTitle>{t("Instance setup")}</CardTitle>
                <CardDescription>
                  {ready
                    ? t("Your instance is ready to use.")
                    : t("This page updates automatically while we set things up.")}
                </CardDescription>
              </CardHeader>
              <CardContent>
                {detail.provisioning_steps.length === 0 && ready
                  ? (
                    <div className="flex items-center gap-3 text-sm">
                      <Check className="text-success size-5" />
                      {t("Setup complete")}
                    </div>
                  )
                  : (
                    <ol className="grid gap-4">
                      {detail.provisioning_steps.map((step) => (
                        <li key={step.step} className="flex items-center gap-3">
                          <StepIcon status={step.status} />
                          <span className="flex-1 text-sm font-medium">
                            {t(STEP_LABELS[step.step])}
                          </span>
                          <span className="text-muted-foreground text-xs">
                            {t(
                              step.status === "succeeded"
                                ? "Done"
                                : step.status === "running"
                                ? "In progress"
                                : step.status === "failed"
                                ? "Needs attention"
                                : "Waiting",
                            )}
                          </span>
                        </li>
                      ))}
                    </ol>
                  )}
              </CardContent>
            </Card>
          </>
        )}
        <DeleteInstance
          tenant={detail.tenant}
          onChanged={() => setRevision((value) => value + 1)}
        />
      </div>
    </div>
  );
}

type IssuedPassword = { username: string; email: string; password: string };

function AdminAccount(
  { tenantId, domain, username, email, ready, passwordIssued, waitingForDomain }: {
    tenantId: string;
    domain: string;
    username: string;
    email: string;
    ready: boolean;
    passwordIssued: boolean;
    waitingForDomain: boolean;
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
      disabled={!ready || busy}
      onClick={passwordIssued ? undefined : () => void issue()}
    >
      {busy ? <Spinner label={t("In progress")} /> : <KeyRound />}
      {passwordIssued ? t("Reset password") : t("Create password")}
    </Button>
  );

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("Admin account")}</CardTitle>
        <CardDescription>
          {ready
            ? t("Sign in to your instance as its owner to manage it.")
            : waitingForDomain
            ? t("Your admin account is created after your custom domain is connected.")
            : t("Your admin account is created when setup finishes.")}
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <div className="grid gap-4 sm:grid-cols-3">
          <div className="grid gap-1">
            <span className="text-muted-foreground text-xs">
              {t("Sign-in email")}
            </span>
            <code className="break-all">{email}</code>
          </div>
          <div className="grid gap-1">
            <span className="text-muted-foreground text-xs">{t("Username")}</span>
            <code className="break-all">@{username}</code>
          </div>
          {ready && (
            <div className="grid gap-1">
              <span className="text-muted-foreground text-xs">{t("Sign-in page")}</span>
              <a
                className="break-all text-sm underline-offset-4 hover:underline"
                href={loginUrl}
                target="_blank"
                rel="noreferrer"
              >
                {loginUrl}
              </a>
            </div>
          )}
        </div>

        {issued && (
          <div className="bg-muted/60 grid gap-3 rounded-md p-3 text-sm">
            <div className="grid gap-1">
              <span className="text-muted-foreground text-xs">
                {t("Sign-in email")}
              </span>
              <code className="break-all select-all">{issued.email}</code>
            </div>
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

        {error && (
          <Alert variant="destructive">
            <CircleAlert />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}

        <div>
          {passwordIssued && ready && !busy
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
      </CardContent>
    </Card>
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
        location.assign("/");
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
    <Card>
      <CardHeader>
        <CardTitle>{retained ? t("Deleted instance") : t("Delete instance")}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        {(retained || state === "deprovisioning") && tenant.purge_scheduled_for && (
          <div className="grid gap-1">
            <span className="text-muted-foreground text-xs">
              {t("Permanently deleted on")}
            </span>
            <span className="text-sm font-medium">
              {absoluteTime(tenant.purge_scheduled_for)}
            </span>
          </div>
        )}
        {error && (
          <Alert variant="destructive">
            <CircleAlert />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}
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
      </CardContent>
    </Card>
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
    <Card>
      <CardHeader>
        <CardTitle>{t("Custom domain setup")}</CardTitle>
        <CardDescription>
          {t("Custom domains can only be added when creating an instance.")}
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-5">
        {domains.length === 0 && (
          <p className="text-muted-foreground text-sm">
            {t("No custom domain was added for this instance.")}
          </p>
        )}
        {domains.map((domain) => (
          <section key={domain.hostname} className="grid gap-3">
            <div className="flex items-center justify-between gap-3">
              <strong className="break-all">{domain.hostname}</strong>
              <Badge variant={domain.status === "active" ? "secondary" : "outline"}>
                {domain.status === "active"
                  ? t("Connected")
                  : domain.status === "removing"
                  ? t("Removing")
                  : t("Waiting for DNS")}
              </Badge>
            </div>
            {domain.status === "active" && (
              <>
                <p className="text-muted-foreground text-sm">
                  {t(
                    "Your custom domain is connected. Keep this CNAME record at your DNS provider; without it, your instance can't be reached at this domain.",
                  )}
                </p>
                <DnsRecord
                  type="CNAME"
                  name={domain.hostname}
                  value={domain.cname_target}
                />
              </>
            )}
            {domain.status === "removing" && (
              <p className="text-muted-foreground text-sm">
                {t("This domain wasn't connected in time and is being released.")}
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
                  <div className="grid gap-1">
                    <span className="text-muted-foreground text-xs">
                      {t("If it isn't connected by then, this instance is deleted")}
                    </span>
                    <span className="text-sm font-medium">
                      {absoluteTime(domain.setup_deadline)}
                    </span>
                  </div>
                )}
              </>
            )}
          </section>
        ))}
      </CardContent>
    </Card>
  );
}

function DnsRecord(
  { type, name, value }: { type: string; name: string; value: string },
) {
  const { i18n } = useLingui();
  return (
    <div className="bg-muted/60 grid gap-3 rounded-md p-3 text-sm">
      <div className="grid gap-1">
        <span className="text-muted-foreground text-xs">{i18n._("Type")}</span>
        <code>{type}</code>
      </div>
      <div className="grid gap-1">
        <span className="text-muted-foreground text-xs">{i18n._("Name")}</span>
        <code className="break-all">{name}</code>
      </div>
      <div className="grid gap-1">
        <span className="text-muted-foreground text-xs">{i18n._("Value")}</span>
        <code className="break-all">{value}</code>
      </div>
    </div>
  );
}

function StepIcon({ status }: { status: CustomerProvisioningStep["status"] }) {
  const { i18n } = useLingui();
  if (status === "succeeded") return <Check className="text-success size-5" />;
  if (status === "running") {
    return (
      <Spinner label={i18n._("In progress")} className="text-pending size-5" />
    );
  }
  if (status === "failed") {
    return <CircleAlert className="text-destructive size-5" />;
  }
  return <Clock className="text-muted-foreground size-5" />;
}
