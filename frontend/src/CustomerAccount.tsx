import { MemberUsage } from "@/components/customer/usage";
import { useState } from "react";
import { useLingui } from "@lingui/react";
import { KeyRound } from "lucide-react";
import { api, type Customer, errorMessage } from "@/api";
import { navigate } from "@/navigation";
import { Button } from "@/components/ui/button";
import { ErrorAlert, Page, PageHeader, SectionCard, SuccessAlert } from "@/components/customer/page";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { SubscriptionSections } from "@/CustomerSubscription";
import { setLocale, type Locale } from "@/i18n";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";

export function CustomerAccount(
  { customer, onChanged }: { customer: Customer; onChanged: () => Promise<void> },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  return (
    <Page>
      <PageHeader title={t("Account")} description={customer.email} />
      {customer.has_password && <SubscriptionSections customer={customer} onChanged={onChanged} />}
      <PasswordCard customer={customer} onChanged={onChanged} />
      <LanguageCard />
      <MemberUsage />
    </Page>
  );
}

function LanguageCard() {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  return (
    <SectionCard
      title={t("Language")}
      description={t("Choose the language for this site and the emails we send you.")}
    >
      <ToggleGroup
        variant="outline"
        size="sm"
        value={[i18n.locale]}
        onValueChange={(value) => {
          const next = value[0] as Locale | undefined;
          if (next && next !== i18n.locale) void setLocale(next);
        }}
        aria-label={t("Language")}
      >
        {/* Each language is named in itself, so either reader finds theirs. */}
        <ToggleGroupItem value="ko" lang="ko">한국어</ToggleGroupItem>
        <ToggleGroupItem value="en" lang="en">English</ToggleGroupItem>
      </ToggleGroup>
    </SectionCard>
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
  // A first password is the point of the card; a change waits to be asked for.
  const [editing, setEditing] = useState(false);
  const first = !customer.has_password;
  const showForm = first || editing;
  function cancel() {
    setCurrent("");
    setNext("");
    setError("");
    setEditing(false);
  }

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
        await onChanged().catch(() => {});
        navigate("/instances");
        return;
      }
      setCurrent("");
      setNext("");
      setChanged(true);
      setEditing(false);
      await onChanged();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <SectionCard
      highlight={first}
      title={first
        ? t("Set your password")
        : editing
        ? t("Change your password")
        : t("Password")}
      description={first
        ? t("Your email is verified. Add a password of at least 14 characters for password sign-in.")
        : t("A new password of at least 14 characters signs out every other device.")}
    >
        {showForm
          ? (
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
              {error && <ErrorAlert message={error} />}
              <div className="flex flex-wrap gap-2">
                <Button type="submit" disabled={busy}>
                  {busy
                    ? t("Please wait…")
                    : first
                    ? t("Set password")
                    : t("Change password")}
                </Button>
                {!first && (
                  <Button type="button" variant="outline" disabled={busy} onClick={cancel}>
                    {t("Cancel")}
                  </Button>
                )}
              </div>
            </form>
          )
          : (
            <>
              {changed && <SuccessAlert>{t("Your password was changed.")}</SuccessAlert>}
              <div>
                <Button
                  variant="outline"
                  onClick={() => {
                    setChanged(false);
                    setEditing(true);
                  }}
                >
                  <KeyRound />
                  {t("Change password")}
                </Button>
              </div>
            </>
          )}
    </SectionCard>
  );
}
