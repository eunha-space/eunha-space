import { useEffect, useState } from "react";
import { CircleAlert, LogOut, Plus, UserRound } from "lucide-react";
import {
  api,
  countsTowardInstanceLimit,
  type Customer,
  errorMessage,
  instanceAddress,
  instanceStatus,
  type Tenant,
} from "@/api";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
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
import { CustomerInstanceDetail } from "@/CustomerInstanceDetail";
import { CustomerAccount } from "@/CustomerAccount";
import { CustomerCreateInstance } from "@/CustomerCreateInstance";

export function CustomerApp() {
  const { i18n } = useLingui();
  const t = (message: string, values?: Record<string, string | number>) =>
    i18n._(message, values);
  const [customer, setCustomer] = useState<Customer | null>(null);
  const [sessionLoaded, setSessionLoaded] = useState(false);
  const [tenants, setTenants] = useState<Tenant[]>([]);
  const [signup, setSignup] = useState(false);
  const [sent, setSent] = useState(false);
  const [verified] = useState(
    () => new URLSearchParams(location.search).get("verified") === "1",
  );
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  async function load() {
    const me = await api<Customer>("/customer/session/me");
    const nextTenants = await api<Tenant[]>("/customer/tenants");
    setCustomer(me);
    setTenants(nextTenants);
  }
  useEffect(() => {
    const token = new URLSearchParams(location.search).get("token");
    if (location.pathname === "/verify" && token) {
      void api(`/customer/verify?token=${encodeURIComponent(token)}`)
        .then(() => location.replace(`${location.origin}/?verified=1`))
        .catch((e) => setError(errorMessage(e)));
    }
    load().catch(() => {}).finally(() => setSessionLoaded(true));
  }, []);
  async function authenticate(event: React.FormEvent) {
    event.preventDefault();
    setError("");
    setBusy(true);
    try {
      if (signup) {
        await api("/customer/register", {
          method: "POST",
          body: JSON.stringify({ email }),
        });
        setSent(true);
      } else {
        await api("/customer/session", {
          method: "POST",
          body: JSON.stringify({ email, password }),
        });
        await load();
      }
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }
  async function emailSignIn() {
    setBusy(true);
    setError("");
    try {
      await api("/customer/session", {
        method: "POST",
        body: JSON.stringify({ email }),
      });
      setSent(true);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }
  async function signOut() {
    await api("/customer/session", { method: "DELETE" });
    setCustomer(null);
    setTenants([]);
  }
  const activeInstances = tenants.filter(countsTowardInstanceLimit).length;
  const reachedInstanceLimit = customer?.instance_limit != null &&
    activeInstances >= customer.instance_limit;
  if (!sessionLoaded) {
    return (
      <main className="grid min-h-screen place-items-center p-4">
        <div className="grid justify-items-center gap-4">
          <Mark />
          <Spinner label={t("Loading…")} />
        </div>
      </main>
    );
  }
  if (!customer) {
    return (
      <main className="grid min-h-screen place-items-center p-4">
        <div className="fixed top-5 right-5">
          <div className="flex gap-2">
            <LanguageToggle />
            <ThemeToggle />
          </div>
        </div>
        <Card className="w-full max-w-md">
          <CardHeader>
            <Mark className="mb-4" />
            <CardTitle>
              {t(signup ? "Create your account" : "Your eunha space instances")}
            </CardTitle>
            <CardDescription>
              {sent
                ? t("Check your inbox and follow the verification link.")
                : signup
                ? t("Verify your email before creating up to two instances.")
                : t("Sign in to create and manage your instances.")}
            </CardDescription>
          </CardHeader>
          {!sent && (
            <CardContent>
              {verified && (
                <Alert className="mb-4">
                  <AlertDescription>
                    {t("Email verified. Sign in to continue.")}
                  </AlertDescription>
                </Alert>
              )}
              <form onSubmit={authenticate} className="grid gap-4">
                <div className="grid gap-2">
                  <Label htmlFor="email">{t("Email")}</Label>
                  <Input
                    id="email"
                    type="email"
                    autoComplete="email"
                    value={email}
                    onChange={(e) => setEmail(e.target.value)}
                    required
                  />
                </div>
                {!signup && (
                  <div className="grid gap-2">
                    <Label htmlFor="password">{t("Password")}</Label>
                    <Input
                      id="password"
                      type="password"
                      autoComplete="current-password"
                      value={password}
                      onChange={(e) => setPassword(e.target.value)}
                      required
                    />
                  </div>
                )}
                {error && (
                  <Alert variant="destructive">
                    <CircleAlert />
                    <AlertDescription>{error}</AlertDescription>
                  </Alert>
                )}
                <Button type="submit" disabled={busy}>
                  {busy
                    ? t("Please wait…")
                    : signup
                    ? t("Create account")
                    : t("Sign in")}
                </Button>
                {!signup && (
                  <div className="grid gap-2">
                    <Button
                      type="button"
                      variant="outline"
                      disabled={busy || !email}
                      onClick={emailSignIn}
                    >
                      {t("Email me a sign-in link")}
                    </Button>
                    <span className="text-muted-foreground text-xs">
                      {t(
                        "Forgot your password? Sign in with an email link, then set a new one.",
                      )}
                    </span>
                  </div>
                )}
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => setSignup(!signup)}
                >
                  {signup
                    ? t("Already have an account? Sign in")
                    : t("New here? Create an account")}
                </Button>
              </form>
            </CardContent>
          )}
        </Card>
      </main>
    );
  }
  if (location.pathname === "/account") {
    return (
      <main className="mx-auto min-h-screen max-w-2xl p-4 md:py-10">
        <CustomerHeader customer={customer} onSignOut={signOut} />
        <CustomerAccount customer={customer} onChanged={load} />
      </main>
    );
  }
  if (location.pathname === "/instances/new" && customer.has_password) {
    return (
      <main className="mx-auto min-h-screen max-w-2xl p-4 md:py-10">
        <CustomerHeader customer={customer} onSignOut={signOut} />
        <CustomerCreateInstance
          customer={customer}
          activeInstances={activeInstances}
        />
      </main>
    );
  }
  const instanceId = location.pathname.match(/^\/instances\/([^/]+)$/)?.[1];
  if (instanceId) {
    return (
      <main className="mx-auto min-h-screen max-w-2xl p-4 md:py-10">
        <CustomerHeader customer={customer} onSignOut={signOut} />
        <CustomerInstanceDetail id={instanceId} />
      </main>
    );
  }
  return (
    <main className="mx-auto min-h-screen max-w-2xl p-4 md:py-10">
      <CustomerHeader customer={customer} onSignOut={signOut} />
      {error && (
        <Alert variant="destructive" className="mb-4">
          <CircleAlert />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      {!customer.has_password && (
        <Card className="border-primary/40 mb-6">
          <CardHeader>
            <CardTitle>{t("Set your password")}</CardTitle>
            <CardDescription>
              {t(
                "Your email is verified. Add a password of at least 14 characters for password sign-in.",
              )}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Button render={<a href="/account" />}>{t("Set password")}</Button>
          </CardContent>
        </Card>
      )}
      {customer.has_password && (
        <Card>
          <CardHeader>
            <CardTitle>{t("Your instances")}</CardTitle>
            {customer.instance_limit != null && (
              <CardDescription>
                {i18n.locale === "ko"
                  ? `${customer.instance_limit}개 중 ${activeInstances}개 사용 중입니다.`
                  : `${activeInstances} of ${customer.instance_limit} used.`}
              </CardDescription>
            )}
            {!reachedInstanceLimit && (
              <CardAction>
                <Button render={<a href="/instances/new" />}>
                  <Plus /> {t("Create an instance")}
                </Button>
              </CardAction>
            )}
          </CardHeader>
          <CardContent className="grid gap-3">
            {tenants.length === 0
              ? (
                <p className="text-muted-foreground text-sm">
                  {t("No instances yet.")}
                </p>
              )
              : tenants.map((t) => (
                <a
                  key={t.id}
                  href={`/instances/${t.id}`}
                  className="bg-muted/50 hover:bg-muted rounded-md p-3 transition-colors"
                >
                  <strong>{t.title}</strong>
                  <p className="text-muted-foreground text-sm">
                    {instanceAddress(t)} · {i18n._(instanceStatus(t))}
                  </p>
                </a>
              ))}
          </CardContent>
        </Card>
      )}
    </main>
  );
}

function CustomerHeader(
  { customer, onSignOut }: {
    customer: Customer;
    onSignOut: () => Promise<void>;
  },
) {
  const { i18n } = useLingui();
  return (
    <header className="mb-8 flex items-center justify-between">
      <div className="flex items-center gap-3">
        <Mark />
        <div>
          <strong>eunha space</strong>
          <p className="text-muted-foreground text-sm">{customer.email}</p>
        </div>
      </div>
      <div className="flex gap-2">
        <LanguageToggle />
        <ThemeToggle />
        <Button
          variant="outline"
          size="icon"
          aria-label={i18n._("Account")}
          render={<a href="/account" />}
        >
          <UserRound />
        </Button>
        <Button variant="outline" size="icon" onClick={() => void onSignOut()}>
          <LogOut />
        </Button>
      </div>
    </header>
  );
}
