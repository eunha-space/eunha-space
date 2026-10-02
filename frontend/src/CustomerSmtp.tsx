import { useEffect, useState } from "react";
import { useLingui } from "@lingui/react";
import { api, errorMessage } from "@/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from "@/components/ui/card";

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
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    api<{ host: string | null; port: number | null; from: string | null }>(`/customer/tenants/${tenantId}/smtp`)
      .then((settings) => {
        if (!active) return;
        setHost(settings.host ?? ""); setPort(String(settings.port ?? 587)); setFrom(settings.from ?? ""); setLoaded(true);
      }).catch((failure) => active && setError(errorMessage(failure)));
    return () => { active = false; };
  }, [tenantId]);
  return <Card>
    <CardHeader>
      <CardTitle>{t("Email delivery")}</CardTitle>
      <CardDescription>{t("Use your SMTP provider for instance emails. Port 465 uses TLS; port 587 requires STARTTLS. Enter both credentials whenever you save; saved credentials are never displayed.")}</CardDescription>
    </CardHeader>
    <CardContent>
      <form className="grid gap-4" onSubmit={async (event) => {
        event.preventDefault(); setBusy(true); setError(""); setMessage("");
        try {
          await api(`/customer/tenants/${tenantId}/smtp`, { method: "PUT", body: JSON.stringify({ host, port: Number(port), from, username, password }) });
          setUsername(""); setPassword(""); setMessage(t("Saved. Your instance will apply these settings shortly."));
        } catch (failure) { setError(errorMessage(failure)); }
        finally { setBusy(false); }
      }}>
        <div className="grid gap-2"><Label htmlFor="smtp-host">{t("SMTP hostname")}</Label><Input id="smtp-host" value={host} onChange={(e) => setHost(e.target.value)} required maxLength={253} placeholder="smtp.example.com" /></div>
        <div className="grid gap-2"><Label htmlFor="smtp-port">{t("Port")}</Label><Input id="smtp-port" type="number" value={port} onChange={(e) => setPort(e.target.value)} required /></div>
        <div className="grid gap-2"><Label htmlFor="smtp-from">{t("Sender email")}</Label><Input id="smtp-from" type="email" value={from} onChange={(e) => setFrom(e.target.value)} required maxLength={254} /></div>
        <div className="grid gap-2"><Label htmlFor="smtp-username">{t("SMTP username")}</Label><Input id="smtp-username" type="password" autoComplete="off" value={username} onChange={(e) => setUsername(e.target.value)} required maxLength={1024} /></div>
        <div className="grid gap-2"><Label htmlFor="smtp-password">{t("SMTP password")}</Label><Input id="smtp-password" type="password" autoComplete="new-password" value={password} onChange={(e) => setPassword(e.target.value)} required maxLength={4096} /></div>
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        {message && <p role="status" className="text-sm text-muted-foreground">{message}</p>}
        <Button type="submit" disabled={busy || !loaded}>{busy ? t("Saving…") : t("Save SMTP settings")}</Button>
      </form>
    </CardContent>
  </Card>;
}
