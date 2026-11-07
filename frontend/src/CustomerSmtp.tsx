import { useEffect, useState } from "react";
import { useLingui } from "@lingui/react";
import { CircleCheck, Pencil } from "lucide-react";
import { api, errorMessage } from "@/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Detail, ErrorAlert, SectionCard, StateBadge, SuccessAlert } from "@/components/customer/page";

export function CustomerSmtp({ tenantId }: { tenantId: string }) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const [host, setHost] = useState("");
  const [port, setPort] = useState("587");
  const [from, setFrom] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [configured, setConfigured] = useState(false);
  // What is saved, shown while not editing and restored by Cancel.
  const [saved, setSaved] = useState({ host: "", port: "587", from: "" });
  // Nothing saved yet means the form is the only thing to show.
  const [editing, setEditing] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    api<{ configured: boolean; host: string | null; port: number | null; from: string | null }>(
      `/customer/tenants/${tenantId}/smtp`,
    )
      .then((settings) => {
        if (!active) return;
        const current = {
          host: settings.host ?? "",
          port: String(settings.port ?? 587),
          from: settings.from ?? "",
        };
        setSaved(current);
        setHost(current.host);
        setPort(current.port);
        setFrom(current.from);
        setConfigured(settings.configured);
        setLoaded(true);
      })
      .catch((failure) => active && setError(errorMessage(failure)));
    return () => {
      active = false;
    };
  }, [tenantId]);
  async function save(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    setMessage("");
    try {
      await api(`/customer/tenants/${tenantId}/smtp`, {
        method: "PUT",
        body: JSON.stringify({ host, port: Number(port), from, username, password }),
      });
      setUsername("");
      setPassword("");
      setSaved({ host: host.trim().toLowerCase(), port, from });
      setConfigured(true);
      setEditing(false);
      setMessage(t("Saved. Your instance will apply these settings shortly."));
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  }
  function startEditing() {
    setMessage("");
    setError("");
    setEditing(true);
  }
  function cancel() {
    setHost(saved.host);
    setPort(saved.port);
    setFrom(saved.from);
    setUsername("");
    setPassword("");
    setError("");
    setEditing(false);
  }
  const showForm = editing || !configured;
  // Until the settings arrive the card can't say which state it is in, so it
  // waits rather than show a plain card that turns green or red a moment later.
  if (!loaded) {
    return error
      ? (
        <SectionCard title={t("Email delivery")}>
          <ErrorAlert message={error} />
        </SectionCard>
      )
      : null;
  }
  return (
    <SectionCard
      highlight={!configured}
      tone={configured ? "success" : undefined}
      title={t("Email delivery")}
      badge={configured
        ? (
          <StateBadge tone="success">
            <CircleCheck />
            {t("Configured")}
          </StateBadge>
        )
        : <StateBadge tone="problem">{t("Not configured")}</StateBadge>}
      description={configured
        ? editing
          ? t("Enter the credentials again whenever you change these settings; saved credentials are never displayed.")
          : t("Your instance sends email through this SMTP server.")
        : t(
          "Custom domains require your own SMTP credentials. Email delivery is unavailable until you save them.",
        )}
    >
      {showForm
        ? (
          <form className="grid gap-4" onSubmit={save}>
            <div className="grid gap-2">
              <Label htmlFor="smtp-host">{t("SMTP hostname")}</Label>
              <Input
                id="smtp-host"
                value={host}
                onChange={(e) => setHost(e.target.value)}
                required
                maxLength={253}
                placeholder="smtp.example.com"
              />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="smtp-port">{t("Port")}</Label>
              <Input
                id="smtp-port"
                type="number"
                value={port}
                onChange={(e) => setPort(e.target.value)}
                required
              />
              <span className="text-muted-foreground text-xs">
                {t("Port 465 uses TLS; port 587 requires STARTTLS.")}
              </span>
            </div>
            <div className="grid gap-2">
              <Label htmlFor="smtp-from">{t("Sender email")}</Label>
              <Input
                id="smtp-from"
                type="email"
                value={from}
                onChange={(e) => setFrom(e.target.value)}
                required
                maxLength={254}
              />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="smtp-username">{t("SMTP username")}</Label>
              <Input
                id="smtp-username"
                type="password"
                autoComplete="off"
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                required
                maxLength={1024}
              />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="smtp-password">{t("SMTP password")}</Label>
              <Input
                id="smtp-password"
                type="password"
                autoComplete="new-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                required
                maxLength={4096}
              />
            </div>
            {error && <ErrorAlert message={error} />}
            <div className="flex flex-wrap gap-2">
              <Button type="submit" disabled={busy}>
                {busy ? t("Saving…") : t("Save SMTP settings")}
              </Button>
              {configured && (
                <Button type="button" variant="outline" disabled={busy} onClick={cancel}>
                  {t("Cancel")}
                </Button>
              )}
            </div>
          </form>
        )
        : (
          <>
            <dl className="grid gap-4 sm:grid-cols-2">
              <Detail label={t("SMTP server")}>{saved.host}:{saved.port}</Detail>
              <Detail label={t("Sender email")}>{saved.from}</Detail>
            </dl>
            {message && <SuccessAlert>{message}</SuccessAlert>}
            <div>
              <Button variant="outline" onClick={startEditing}>
                <Pencil />
                {t("Change settings")}
              </Button>
            </div>
          </>
        )}
    </SectionCard>
  );
}
