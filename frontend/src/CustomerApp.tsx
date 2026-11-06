import { useEffect, useRef, useState } from "react";
import { CreditCard, KeyRound, Plus, Server } from "lucide-react";
import {
  api,
  ApiError,
  takeBoot,
  countsTowardInstanceLimit,
  type Customer,
  errorMessage,
  instanceAddress,
  type Tenant,
} from "@/api";
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
import { EmptyState } from "@/components/console/status";
import {
  ActionAlert,
  ErrorAlert,
  Loading,
  Page,
  PageHeader,
  siteColumn,
  SuccessAlert,
} from "@/components/customer/page";
import { InstanceStatusBadge } from "@/components/customer/instance";
import { instanceUsage } from "@/format";
import { useLingui } from "@lingui/react";
import { CustomerInstanceDetail } from "@/CustomerInstanceDetail";
import { CustomerAccount } from "@/CustomerAccount";
import { CustomerCreateInstance } from "@/CustomerCreateInstance";
import { Landing } from "@/Landing";
import { Pricing } from "@/Pricing";
import { RefundPolicy } from "@/RefundPolicy";
import { CommunityGuidelines } from "@/CommunityGuidelines";
import { SiteHeader } from "@/SiteHeader";
import { navigate, useCustomerNavigation } from "@/navigation";

const PUBLIC_PAGES = ["/", "/pricing", "/refund-policy", "/community-guidelines"];

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
    // Both at once: a signed-out visitor's tenants fail as the session does.
    // The first session check is the one index.html has already started.
    const [me, nextTenants] = await Promise.all([
      takeBoot<Customer>("me") ?? api<Customer>("/customer/session/me"),
      // The pages for anyone list no instances.
      PUBLIC_PAGES.includes(location.pathname)
        ? Promise.resolve(null)
        : api<Tenant[]>("/customer/tenants"),
    ]);
    setCustomer(me);
    if (nextTenants) setTenants(nextTenants);
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
    // Signing out of a masquerade returns to the administrator's session.
    const masquerading = customer?.masquerading_admin;
    await api("/customer/session", { method: "DELETE" });
    location.assign(masquerading ? "/admin/users" : "/");
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
  // The pages for anyone say the same thing signed in or not, so they do not
  // wait for the session; the header fills in its account when it is known.
  if (location.pathname === "/") return withHeader(<Landing />);
  if (location.pathname === "/pricing") return withHeader(<Pricing customer={sessionLoaded ? customer : undefined} />);
  if (location.pathname === "/refund-policy") return withHeader(<RefundPolicy />);
  if (location.pathname === "/community-guidelines") return withHeader(<CommunityGuidelines />);
  if (!sessionLoaded) {
    return withHeader(
      <Page>
        <Loading />
      </Page>,
    );
  }
  if (!customer) {
    return withHeader(
      // A form on its own, centred; at a fixed height, so that it stays put
      // when it switches between signing in and signing up.
      <main className={`${siteColumn} grid flex-1 content-start justify-items-center py-8 md:py-12`}>
        <Card className="w-full max-w-md">
          <CardHeader>
            <CardTitle>
              <h1>{t(signup ? "Create your account" : "Sign in")}</h1>
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
                <div className="mb-4">
                  <SuccessAlert>{t("Email verified. Sign in to continue.")}</SuccessAlert>
                </div>
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
                {error && <ErrorAlert message={error} />}
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
    return withHeader(<CustomerAccount customer={customer} onChanged={load} />);
  }
  if (
    location.pathname === "/instances/new" && customer.has_password &&
    !customer.subscription_required
  ) {
    return withHeader(
      <CustomerCreateInstance
        customer={customer}
        activeInstances={activeInstances}
      />,
    );
  }
  const instanceId = location.pathname.match(/^\/instances\/([^/]+)$/)?.[1];
  // Without a password or a subscription there is no form yet, only the list
  // saying what to do first.
  if (instanceId && instanceId !== "new") {
    return withHeader(<CustomerInstanceDetail key={instanceId} id={instanceId} />);
  }
  const canCreate = customer.has_password && !reachedInstanceLimit &&
    !customer.subscription_required;
  return withHeader(
    <Page>
      <PageHeader
        title={t("Your instances")}
        description={customer.has_password
          ? i18n._(instanceUsage(i18n.locale, activeInstances, customer.instance_limit))
          : undefined}
        action={canCreate && (
          <Button render={<a href="/instances/new" />}>
            <Plus /> {t("Create an instance")}
          </Button>
        )}
      />
      {error && <ErrorAlert message={error} />}
      {!customer.has_password && (
        <ActionAlert icon={<KeyRound />} href="/account" action={t("Set password")}>
          {t("Your email is verified. Add a password of at least 14 characters for password sign-in.")}
        </ActionAlert>
      )}
      {customer.has_password && customer.subscription_required && (
        <ActionAlert icon={<CreditCard />} href="/account" action={t("Subscribe")}>
          {t("Subscribe to create and run instances.")}
        </ActionAlert>
      )}
      {customer.has_password && (
        <Card className="py-0">
          {tenants.length === 0
            ? <EmptyState icon={<Server />}>{t("No instances yet.")}</EmptyState>
            : (
              <ul className="divide-y">
                {tenants.map((tenant) => (
                  <li key={tenant.id}>
                    <a
                      href={`/instances/${tenant.id}`}
                      className="hover:bg-muted flex items-center justify-between gap-3 px-4 py-3 transition-colors"
                    >
                      <span className="min-w-0 font-medium wrap-anywhere">
                        {instanceAddress(tenant)}
                      </span>
                      <InstanceStatusBadge tenant={tenant} />
                    </a>
                  </li>
                ))}
              </ul>
            )}
        </Card>
      )}
    </Page>,
  );
}
