import React, { useState } from "react";
import { CircleAlert } from "lucide-react";
import { api, errorMessage } from "@/api";
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
import { Mark, ThemeToggle } from "@/components/console/brand";
import { Spinner } from "@/components/console/status";
import { useLingui } from "@lingui/react";
import { LanguageToggle } from "@/i18n";

export function Login({ done }: { done: () => void }) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await api("/session", {
        method: "POST",
        body: JSON.stringify({ username, password }),
      });
      done();
    } catch (failure) {
      setError(errorMessage(failure));
      setBusy(false);
    }
  }

  return (
    <main className="grid min-h-screen place-items-center p-4">
      <div className="fixed top-5 right-5">
        <div className="flex gap-2">
          <LanguageToggle />
          <ThemeToggle />
        </div>
      </div>
      <Card className="w-full max-w-sm">
        <CardHeader>
          <Mark className="mb-4" />
          <CardTitle className="text-2xl tracking-tight">
            {t("Control plane")}
          </CardTitle>
          <CardDescription>
            {t("Sign in to manage the Eunha runtime fleet.")}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={submit} className="grid gap-4">
            <div className="grid gap-2">
              <Label htmlFor="operator">{t("Operator")}</Label>
              <Input
                id="operator"
                value={username}
                autoComplete="username"
                autoFocus
                required
                onChange={(event) => setUsername(event.target.value)}
              />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="password">{t("Password")}</Label>
              <Input
                id="password"
                type="password"
                value={password}
                autoComplete="current-password"
                required
                onChange={(event) => setPassword(event.target.value)}
              />
            </div>
            {error && (
              <Alert variant="destructive">
                <CircleAlert />
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
            <Button type="submit" disabled={busy} className="w-full">
              {busy && <Spinner label="Signing in" />}
              {busy ? t("Signing in…") : t("Sign in")}
            </Button>
          </form>
        </CardContent>
      </Card>
    </main>
  );
}
