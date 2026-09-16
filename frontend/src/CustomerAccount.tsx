import { useState } from "react";
import { ArrowLeft, CircleAlert } from "lucide-react";
import { useLingui } from "@lingui/react";
import { api, type Customer, errorMessage } from "@/api";
import { Alert, AlertDescription } from "@/components/ui/alert";
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

export function CustomerAccount(
  { customer, onChanged }: { customer: Customer; onChanged: () => Promise<void> },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  return (
    <div className="grid gap-6">
      <div>
        <Button variant="ghost" className="mb-3 -ml-3" render={<a href="/" />}>
          <ArrowLeft /> {t("All instances")}
        </Button>
        <h1 className="text-2xl font-semibold tracking-tight">{t("Account")}</h1>
        <p className="text-muted-foreground text-sm">{customer.email}</p>
      </div>
      <PasswordCard customer={customer} onChanged={onChanged} />
    </div>
  );
}

function PasswordCard(
  { customer, onChanged }: { customer: Customer; onChanged: () => Promise<void> },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [changed, setChanged] = useState(false);
  const first = !customer.has_password;

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setError("");
    setChanged(false);
    if (next.length < 14) {
      setError(t("Use a password with at least 14 characters."));
      return;
    }
    setBusy(true);
    try {
      await api("/customer/password", {
        method: "POST",
        body: JSON.stringify({
          password: next,
          current_password: customer.requires_current_password ? current : null,
        }),
      });
      if (first) {
        // A first password is what the instance list was waiting on, so hand
        // the customer straight back to it rather than leave them here.
        location.assign("/");
        return;
      }
      setCurrent("");
      setNext("");
      setChanged(true);
      await onChanged();
    } catch (e) {
      // Server messages are catalog keys too, so they localize like the rest.
      setError(t(errorMessage(e)));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card className={first ? "border-primary/40" : undefined}>
      <CardHeader>
        <CardTitle>
          {first ? t("Set your password") : t("Change your password")}
        </CardTitle>
        <CardDescription>
          {first
            ? t(
              "Your email is verified. Add a password of at least 14 characters for password sign-in.",
            )
            : t(
              "A new password of at least 14 characters signs out every other device.",
            )}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form onSubmit={submit} className="grid gap-4">
          {customer.requires_current_password && (
            <div className="grid gap-2">
              <Label htmlFor="current-password">{t("Current password")}</Label>
              <Input
                id="current-password"
                type="password"
                autoComplete="current-password"
                value={current}
                onChange={(e) => setCurrent(e.target.value)}
                required
              />
            </div>
          )}
          <div className="grid gap-2">
            <Label htmlFor="new-password">
              {first ? t("Password") : t("New password")}
            </Label>
            <Input
              id="new-password"
              type="password"
              autoComplete="new-password"
              value={next}
              onChange={(e) => setNext(e.target.value)}
              required
            />
          </div>
          {error && (
            <Alert variant="destructive">
              <CircleAlert />
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          {changed && (
            <Alert>
              <AlertDescription>{t("Your password was changed.")}</AlertDescription>
            </Alert>
          )}
          <div>
            <Button type="submit" disabled={busy}>
              {busy
                ? t("Please wait…")
                : first
                ? t("Set password")
                : t("Change password")}
            </Button>
          </div>
        </form>
      </CardContent>
    </Card>
  );
}
