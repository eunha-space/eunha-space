import { useState } from "react";
import { Check, CircleAlert, Clock, Globe, X } from "lucide-react";
import { useLingui } from "@lingui/react";
import {
  api,
  type CustomerTenantDetail,
  type DomainChange,
  errorMessage,
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
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Spinner } from "@/components/console/status";

// A change is six steps; a customer follows four stages of it.
const STAGES: { label: string; steps: DomainChange["step"][] }[] = [
  { label: "Connecting the new domain", steps: ["attach"] },
  { label: "Moving your instance", steps: ["rename", "switch"] },
  { label: "Moving your followers", steps: ["move", "deliver"] },
  { label: "Releasing the old domain", steps: ["release"] },
];

const ORDER: DomainChange["step"][] = [
  "attach",
  "rename",
  "switch",
  "move",
  "deliver",
  "release",
  "done",
];

function stageStatus(change: DomainChange, steps: DomainChange["step"][]) {
  const at = ORDER.indexOf(change.step);
  const last = Math.max(...steps.map((step) => ORDER.indexOf(step)));
  const first = Math.min(...steps.map((step) => ORDER.indexOf(step)));
  if (at > last || change.status === "succeeded") return "succeeded";
  if (at < first) return "pending";
  return change.status === "failed" ? "failed" : "running";
}

/** Where an instance's address is, and moving it somewhere else. */
export function CustomerDomainChange(
  { tenant, change, onChanged }: {
    tenant: CustomerTenantDetail["tenant"];
    change: DomainChange | null;
    onChanged: () => void;
  },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const [editing, setEditing] = useState(false);
  const [kind, setKind] = useState<"provided" | "custom">("provided");
  const [subdomain, setSubdomain] = useState("");
  const [customDomain, setCustomDomain] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const running = change?.status === "running";
  const target = kind === "provided"
    ? `${subdomain || "example"}.eunha.space`
    : customDomain;

  async function start() {
    setConfirming(false);
    setBusy(true);
    setError("");
    try {
      await api(`/customer/tenants/${tenant.id}/domain-change`, {
        method: "POST",
        body: JSON.stringify({
          subdomain: kind === "provided" ? subdomain : "",
          custom_domain: kind === "custom" ? customDomain : null,
        }),
      });
      setEditing(false);
      onChanged();
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  }

  async function cancel() {
    setBusy(true);
    setError("");
    try {
      await api(`/customer/tenants/${tenant.id}/domain-change/cancel`, {
        method: "POST",
      });
      onChanged();
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("Domain")}</CardTitle>
        <CardDescription>
          {running
            ? `${change.from_domain} → ${change.to_domain}`
            : t(
              "Your instance's address. Every account's handle and every post's link are under it.",
            )}
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        {running && <ChangeProgress change={change} />}

        {change && !running && change.finished_at &&
          (change.status === "succeeded"
            ? (
              <p className="text-sm">
                {t("Moved from")} <strong>{change.from_domain}</strong>{" "}
                · {absoluteTime(change.finished_at)}
                {change.given_up
                  ? ` · ${change.given_up} ${
                    t("servers didn't take the move; their users can follow your accounts again by hand.")
                  }`
                  : ""}
              </p>
            )
            : change.status === "failed"
            ? (
              <Alert variant="destructive">
                <CircleAlert />
                <AlertDescription>
                  {change.step === "attach"
                    ? t("The move didn't start: the new domain couldn't be connected. Your instance stays where it is.")
                    : t("The move stopped part of the way. We've been told and will finish it.")}
                </AlertDescription>
              </Alert>
            )
            : null)}

        {error && (
          <Alert variant="destructive">
            <CircleAlert />
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}

        {running && change.step === "attach" && (
          <div>
            <Button variant="outline" disabled={busy} onClick={() => void cancel()}>
              <X /> {t("Stop the move")}
            </Button>
          </div>
        )}

        {!running && tenant.lifecycle_state === "ready" && !editing && (
          <div>
            <Button variant="outline" onClick={() => setEditing(true)}>
              <Globe /> {t("Change domain")}
            </Button>
          </div>
        )}

        {editing && !running && (
          <form
            className="grid gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              setConfirming(true);
            }}
          >
            <ToggleGroup
              variant="outline"
              size="sm"
              value={[kind]}
              onValueChange={(next) =>
                setKind((next[0] as "provided" | "custom" | undefined) ?? kind)}
            >
              <ToggleGroupItem value="provided">
                {t("eunha space address")}
              </ToggleGroupItem>
              <ToggleGroupItem value="custom">{t("My own domain")}</ToggleGroupItem>
            </ToggleGroup>
            {kind === "provided"
              ? (
                <div className="grid gap-2">
                  <Label htmlFor="new-subdomain">{t("Eunha hostname")}</Label>
                  <Input
                    id="new-subdomain"
                    placeholder="example"
                    pattern="[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?"
                    value={subdomain}
                    onChange={(event) => setSubdomain(event.target.value.toLowerCase())}
                    required
                  />
                  <span className="text-muted-foreground text-xs">{target}</span>
                </div>
              )
              : (
                <div className="grid gap-2">
                  <Label htmlFor="new-domain">{t("Custom domain")}</Label>
                  <Input
                    id="new-domain"
                    placeholder="social.example.com"
                    value={customDomain}
                    onChange={(event) => setCustomDomain(event.target.value.toLowerCase())}
                    required
                  />
                  <span className="text-muted-foreground text-xs">
                    {t(
                      "Add a CNAME record pointing it to customers.eunha.space. Nothing moves until it's connected, and you can stop until then.",
                    )}
                  </span>
                </div>
              )}
            <div className="flex gap-2">
              <Button type="submit" disabled={busy}>
                {busy ? <Spinner label={t("In progress")} /> : <Globe />}
                {t("Move to this domain")}
              </Button>
              <Button type="button" variant="ghost" onClick={() => setEditing(false)}>
                {t("Cancel")}
              </Button>
            </div>
          </form>
        )}

        <AlertDialog open={confirming} onOpenChange={setConfirming}>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>
                {t("Move your instance to")} {target}?
              </AlertDialogTitle>
              <AlertDialogDescription>
                {t(
                  "Every account's followers are asked to follow it at the new domain, and your accounts follow again whoever they follow. Posts move too, but under new links: links to old posts, and replies to them from other servers, stop working. The current domain stops answering once the move begins, and this can't be undone.",
                )}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>{t("Cancel")}</AlertDialogCancel>
              <AlertDialogAction onClick={() => void start()}>
                {t("Move to this domain")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </CardContent>
    </Card>
  );
}

function ChangeProgress({ change }: { change: DomainChange }) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  return (
    <ol className="grid gap-4">
      {STAGES.map((stage) => {
        const status = stageStatus(change, stage.steps);
        const followers = stage.steps.includes("deliver") && status === "running" &&
          change.pending !== null;
        return (
          <li key={stage.label} className="flex items-center gap-3">
            {status === "succeeded"
              ? <Check className="text-success size-5" />
              : status === "running"
              ? <Spinner label={t("In progress")} className="text-pending size-5" />
              : status === "failed"
              ? <CircleAlert className="text-destructive size-5" />
              : <Clock className="text-muted-foreground size-5" />}
            <span className="flex-1 text-sm font-medium">{t(stage.label)}</span>
            <span className="text-muted-foreground text-xs">
              {followers
                ? `${change.pending} ${t("servers to go")}`
                : t(
                  status === "succeeded"
                    ? "Done"
                    : status === "running"
                    ? "In progress"
                    : status === "failed"
                    ? "Needs attention"
                    : "Waiting",
                )}
            </span>
          </li>
        );
      })}
    </ol>
  );
}
