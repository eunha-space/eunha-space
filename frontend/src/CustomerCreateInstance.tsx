import { useState } from "react";
import { ArrowLeft, CircleAlert } from "lucide-react";
import { useLingui } from "@lingui/react";
import { api, type Customer, errorMessage } from "@/api";
import { instanceUsage } from "@/format";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";

export function CustomerCreateInstance(
  { customer, activeInstances }: {
    customer: Customer;
    activeInstances: number;
  },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const [addressKind, setAddressKind] = useState<"provided" | "custom">(
    "provided",
  );
  const [subdomain, setSubdomain] = useState("");
  const [customDomain, setCustomDomain] = useState("");
  const [dnsCheck, setDnsCheck] = useState<
    { hostname: string; points_to_target: boolean | null; target: string } | null
  >(null);
  const [ownerUsername, setOwnerUsername] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const reachedInstanceLimit = customer.instance_limit != null &&
    activeInstances >= customer.instance_limit;

  async function create(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const created = await api<{ id: string }>("/customer/tenants", {
        method: "POST",
        body: JSON.stringify({
          subdomain: addressKind === "provided" ? subdomain : "",
          custom_domain: addressKind === "custom" ? customDomain : null,
          owner_username: ownerUsername,
        }),
      });
      // A new instance is not usable yet, and its detail page is where the
      // setup it needs is followed; stay busy until the browser gets there.
      location.assign(`/instances/${created.id}`);
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  }

  async function checkDns() {
    const hostname = customDomain.trim();
    if (!hostname.includes(".")) return;
    try {
      const result = await api<
        { points_to_target: boolean | null; target: string }
      >(`/customer/dns-check?hostname=${encodeURIComponent(hostname)}`);
      setDnsCheck({ hostname, ...result });
    } catch {
      // Only a hint; the domain is still validated when the instance is created.
      setDnsCheck(null);
    }
  }

  return (
    <div className="grid gap-6">
      <div>
        <Button variant="ghost" className="mb-3 -ml-3" render={<a href="/" />}>
          <ArrowLeft /> {t("All instances")}
        </Button>
        <h1 className="text-2xl font-semibold tracking-tight">
          {t("Create an instance")}
        </h1>
        <p className="text-muted-foreground text-sm">
          {i18n._(instanceUsage(i18n.locale, activeInstances, customer.instance_limit))}
          {" "}{t("Configure site settings inside Eunha after setup.")}
        </p>
      </div>
      {reachedInstanceLimit
        ? (
          <Alert>
            <CircleAlert />
            <AlertDescription>
              {t(
                "You are using every instance your account allows. Delete one to make room for another.",
              )}
            </AlertDescription>
          </Alert>
        )
        : (
          <Card>
            <CardContent>
              <form onSubmit={create} className="grid gap-4">
                <div className="grid gap-2">
                  <Label>{t("Address")}</Label>
                  <ToggleGroup
                    variant="outline"
                    size="sm"
                    // Base UI keeps single-select state as an array and clears it
                    // when the pressed item is pressed again; keep the choice.
                    value={[addressKind]}
                    onValueChange={(next) =>
                      setAddressKind(
                        (next[0] as "provided" | "custom" | undefined) ??
                          addressKind,
                      )}
                  >
                    <ToggleGroupItem value="provided">
                      {t("eunha space address")}
                    </ToggleGroupItem>
                    <ToggleGroupItem value="custom">
                      {t("My own domain")}
                    </ToggleGroupItem>
                  </ToggleGroup>
                </div>
                {addressKind === "provided" && (
                  <div className="grid gap-2">
                    <Label htmlFor="subdomain">{t("Eunha hostname")}</Label>
                    <Input
                      id="subdomain"
                      placeholder="example"
                      pattern="[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?"
                      value={subdomain}
                      onChange={(e) =>
                        setSubdomain(e.target.value.toLowerCase())}
                      required
                    />
                    <span className="text-muted-foreground text-xs">
                      {subdomain || "example"}.eunha.space
                    </span>
                  </div>
                )}
                {addressKind === "custom" && (
                  <div className="grid gap-2">
                    <Label htmlFor="domain">{t("Custom domain")}</Label>
                    <Input
                      id="domain"
                      placeholder="social.example.com"
                      required
                      value={customDomain}
                      onChange={(e) => {
                        setCustomDomain(e.target.value.toLowerCase());
                        setDnsCheck(null);
                      }}
                      onBlur={() => void checkDns()}
                    />
                    <span className="text-muted-foreground text-xs">
                      {t(
                        "Before creating, add a CNAME record pointing it to customers.eunha.space. You can move the instance to another domain later.",
                      )}
                    </span>
                    {dnsCheck?.hostname === customDomain &&
                      dnsCheck.points_to_target === false && (
                      <span className="text-pending text-xs">
                        {t(
                          "We can't see that CNAME record yet. You can still create the instance; setup keeps trying for 7 days, then deletes the instance.",
                        )}
                      </span>
                    )}
                    {dnsCheck?.hostname === customDomain &&
                      dnsCheck.points_to_target === true && (
                      <span className="text-success text-xs">
                        {t("The CNAME record is in place.")}
                      </span>
                    )}
                  </div>
                )}
                <div className="grid gap-2">
                  <Label htmlFor="owner">{t("Admin username")}</Label>
                  <Input
                    id="owner"
                    placeholder="admin"
                    pattern="[A-Za-z0-9_]{1,30}"
                    maxLength={30}
                    autoComplete="off"
                    value={ownerUsername}
                    onChange={(e) => setOwnerUsername(e.target.value)}
                    required
                  />
                  <span className="text-muted-foreground text-xs">
                    @{ownerUsername || "admin"}
                    {" · "}
                    {t("Letters, numbers, and underscores.")}
                  </span>
                </div>
                {error && (
                  <Alert variant="destructive">
                    <CircleAlert />
                    <AlertDescription>{error}</AlertDescription>
                  </Alert>
                )}
                <div>
                  <Button type="submit" disabled={busy}>
                    {busy ? t("Creating…") : t("Create instance")}
                  </Button>
                </div>
              </form>
            </CardContent>
          </Card>
        )}
    </div>
  );
}
