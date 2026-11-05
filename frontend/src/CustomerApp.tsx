import { useEffect, useRef, useState } from "react";
import { CircleAlert, CreditCard, Plus } from "lucide-react";
import {
  api,
  ApiError,
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
import { Mark } from "@/components/console/brand";
import { Spinner } from "@/components/console/status";
import { useLingui } from "@lingui/react";
import { CustomerInstanceDetail } from "@/CustomerInstanceDetail";
import { CustomerAccount } from "@/CustomerAccount";
import { CustomerCreateInstance } from "@/CustomerCreateInstance";
import { Landing } from "@/Landing";
import { Pricing } from "@/Pricing";
import { SiteHeader } from "@/SiteHeader";
import { navigate, useCustomerNavigation } from "@/navigation";

export function CustomerApp() {
  const { i18n } = useLingui();
  const t = (message: string, values?: Record<string, string | number>) =>
    i18n._(message, values);
  const page = useCustomerNavigation();
  const signup = location.pathname === "/signup";
  const verified = new URLSearchParams(location.search).get("verified") === "1";
  const [customer, setCustomer] = useState<Customer | null>(null);
  const [sessionLoaded, setSessionLoaded] = useState(false);
  const [tenants, setTenants] = useState<Tenant[]>([]);
  const [sent, setSent] = useState(false);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  async function load() {
    const me = await api<Customer>("/customer/session/me");
    const nextTenants = await api<Tenant[]>("/customer/tenants");
    setCustomer(me);
    setTenants(nextTenants);
    // Signed in, these paths have nothing to show.
    if (["/signin", "/signup"].includes(location.pathname)) {
      navigate("/instances", { replace: true });
    }
  }
  // Each page is shown with what is known and then brought up to date, as a
  // fresh load of it would be. A session that has ended shows the sign-in form.
  const loadedPage = useRef(page);
  useEffect(() => {
    if (loadedPage.current === page) return;
    loadedPage.current = page;
    setSent(false);
    setError("");
    load().catch((e) => {
      if (e instanceof ApiError && e.status === 401) {
        setCustomer(null);
        setTenants([]);
      }
    });
  }, [page]);
  useEffect(() => {
    const token = new URLSearchParams(location.search).get("token");
    if (location.pathname === "/verify" && token) {
      void api(`/customer/verify?token=${encodeURIComponent(token)}`)
        .then(() => location.replace(`${location.origin}/instances?verified=1`))
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
          // The language the sign-up page is in becomes the account's.
          body: JSON.stringify({ email, locale: i18n.locale }),
        });
        setSent(true);
      } else {
        await api("/customer/session", {
          method: "POST",
          body: JSON.stringify({ email, password }),
        });
        await load();
        setPassword("");
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
    location.assign("/");
  }
  const activeInstances = tenants.filter(countsTowardInstanceLimit).length;
  const reachedInstanceLimit = customer?.instance_limit != null &&
    activeInstances >= customer.instance_limit;
  const header = (
    <SiteHeader customer={sessionLoaded ? customer : undefined} onSignOut={signOut} />
  );
  const withHeader = (main: React.ReactNode) => (
    <>
      {header}
      {main}
    </>
  );
  const portal = (page: React.ReactNode) =>
    withHeader(<main className="mx-auto w-full max-w-2xl flex-1 px-4 pt-4 pb-10">{page}</main>);
  if (!sessionLoaded) {
    return withHeader(
      <main className="grid flex-1 place-items-center p-4">
        <div className="grid justify-items-center gap-4">
          <Mark />
          <Spinner label={t("Loading…")} />
        </div>
      </main>,
    );
  }
  if (location.pathname === "/") return withHeader(<Landing signedIn={customer !== null} />);
  if (location.pathname === "/pricing") return withHeader(<Pricing customer={customer} />);
  if (!customer) {
    return withHeader(
      <main className="grid flex-1 place-items-center p-4">
        <Card className="w-full max-w-md">
          <CardHeader>
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
                  onClick={() => navigate(signup ? "/signin" : "/signup", { replace: true })}
                >
                  {signup
                    ? t("Already have an account? Sign in")
                    : t("New here? Create an account")}
                </Button>
              </form>
            </CardContent>
          )}
        </Card>
      </main>,
    );
  }
  if (location.pathname === "/account" || location.pathname === "/account/billing") {
    return portal(<CustomerAccount customer={customer} onChanged={load} />);
  }
  if (
    location.pathname === "/instances/new" && customer.has_password &&
    !customer.subscription_required
  ) {
    return portal(
      <CustomerCreateInstance
        customer={customer}
        activeInstances={activeInstances}
      />,
    );
  }
  const instanceId = location.pathname.match(/^\/instances\/([^/]+)$/)?.[1];
  if (instanceId) {
    return portal(<CustomerInstanceDetail key={instanceId} id={instanceId} />);
  }
  return portal(
    <>
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
            {!reachedInstanceLimit && !customer.subscription_required && (
              <CardAction>
                <Button render={<a href="/instances/new" />}>
                  <Plus /> {t("Create an instance")}
                </Button>
              </CardAction>
            )}
          </CardHeader>
          <CardContent className="grid gap-3">
            {customer.subscription_required && (
              <Alert>
                <CreditCard />
                <AlertDescription>
                  {t("Subscribe to create and run instances.")}
                </AlertDescription>
                {/* Not in the description, which underlines every link in it. */}
                <div className="col-start-2 mt-2.5">
                  <Button size="sm" render={<a href="/account" />}>
                    {t("Subscribe")}
                  </Button>
                </div>
              </Alert>
            )}
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
                  <strong>{instanceAddress(t)}</strong>
                  <p className="text-muted-foreground text-sm">
                    {i18n._(instanceStatus(t))}
                  </p>
                </a>
              ))}
          </CardContent>
        </Card>
      )}
    </>,
  );
}
