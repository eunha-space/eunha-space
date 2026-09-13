import { useEffect, useState } from "react";
import { ArrowLeft, Check, CircleAlert, Clock } from "lucide-react";
import { useLingui } from "@lingui/react";
import {
  api,
  type CustomerProvisioningStep,
  type CustomerTenantDetail,
  errorMessage,
} from "@/api";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
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
};

export function CustomerInstanceDetail({ id }: { id: string }) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const [detail, setDetail] = useState<CustomerTenantDetail | null>(null);
  const [error, setError] = useState("");

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
  }, [id]);

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
          <Badge variant={ready ? "secondary" : "outline"}>
            {ready ? t("Ready") : t("Setting up")}
          </Badge>
        </div>
        <a
          className="text-muted-foreground hover:text-foreground text-sm underline-offset-4 hover:underline"
          href={`https://${detail.tenant.domain}`}
          target="_blank"
          rel="noreferrer"
        >
          {detail.tenant.domain}
        </a>
      </div>

      <div className="grid gap-6 md:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>{t("Custom domain setup")}</CardTitle>
            <CardDescription>
              {t("Connect a domain you own to this instance.")}
            </CardDescription>
          </CardHeader>
          <CardContent className="grid gap-5">
            {customDomains.length === 0
              ? (
                <p className="text-muted-foreground text-sm">
                  {t("No custom domain was added for this instance.")}
                </p>
              )
              : customDomains.map((domain) => (
                <section key={domain.hostname} className="grid gap-3">
                  <div className="flex items-center justify-between gap-3">
                    <strong>{domain.hostname}</strong>
                    <Badge
                      variant={domain.status === "active"
                        ? "secondary"
                        : "outline"}
                    >
                      {domain.status === "active"
                        ? t("Connected")
                        : t("Waiting for DNS")}
                    </Badge>
                  </div>
                  {domain.status === "active"
                    ? (
                      <p className="text-muted-foreground text-sm">
                        {t("Your custom domain is connected.")}
                      </p>
                    )
                    : (
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
                              {t(
                                "If requested, also add this verification record.",
                              )}
                            </p>
                            <DnsRecord
                              type="TXT"
                              name={domain.ownership_name}
                              value={domain.ownership_value}
                            />
                          </>
                        )}
                      </>
                    )}
                </section>
              ))}
          </CardContent>
        </Card>

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
      </div>
    </div>
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
