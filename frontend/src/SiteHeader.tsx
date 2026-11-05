import { useLingui } from "@lingui/react";
import { LogOut, Server, Shield, UserRound } from "lucide-react";
import type { Customer } from "@/api";
import { Button } from "@/components/ui/button";
import { Mark, ThemeToggle } from "@/components/console/brand";
import { siteColumn } from "@/components/customer/page";
import { LanguageToggle } from "@/i18n";
import { cn } from "cn";

/**
 * The top of every page but the console: where to go, then the site's settings,
 * then the session. `customer` is undefined until the session is known, when the
 * header offers neither signing in nor an account.
 */
export function SiteHeader(
  { customer, onSignOut }: {
    customer: Customer | null | undefined;
    onSignOut: () => Promise<void>;
  },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const path = location.pathname;
  return (
    <div className={siteColumn}>
      <header className="flex items-center justify-between gap-3 py-5">
        <a
          href="/"
          aria-label={t("eunha space")}
          className="flex min-w-0 items-center gap-2 font-semibold"
        >
          <Mark className="size-9 shrink-0" />
          {/* A signed-in header has more to fit on a phone. */}
          <span className={customer ? "hidden sm:inline" : undefined}>{t("eunha space")}</span>
        </a>
        <div className="flex shrink-0 items-center gap-1 sm:gap-2">
          <nav className="flex items-center gap-1 sm:gap-2" aria-label={t("Site")}>
            {/* There is no room on a phone; the footer links here on every page. */}
            <NavLink href="/pricing" label={t("Pricing")} current={path === "/pricing"} hideOnPhone />
            {customer && (
              <>
                <NavLink
                  href="/instances"
                  label={t("Your instances")}
                  icon={<Server />}
                  current={path === "/instances" || path.startsWith("/instances/")}
                />
                <NavLink
                  href="/account"
                  label={t("Account")}
                  icon={<UserRound />}
                  current={path === "/account" || path === "/account/billing"}
                />
              </>
            )}
            {customer?.is_admin && (
              <NavLink href="/admin" label={t("Control plane")} icon={<Shield />} />
            )}
          </nav>
          <LanguageToggle />
          <ThemeToggle />
          {customer === null && (
            <Button variant="outline" render={<a href="/signin" />}>
              {t("Sign in")}
            </Button>
          )}
          {customer && (
            <Button
              variant="outline"
              size="icon"
              aria-label={t("Sign out")}
              title={t("Sign out")}
              onClick={() => void onSignOut()}
            >
              <LogOut />
            </Button>
          )}
        </div>
      </header>
    </div>
  );
}

/** A page the header goes to: its name, shrinking to its icon where space is short. */
function NavLink(
  { href, label, icon, current = false, hideOnPhone = false }: {
    href: string;
    label: string;
    icon?: React.ReactNode;
    current?: boolean;
    hideOnPhone?: boolean;
  },
) {
  return (
    <Button
      variant="ghost"
      className={cn(
        hideOnPhone && "hidden sm:inline-flex",
        icon && "max-md:aspect-square max-md:px-0",
        current && "bg-muted",
      )}
      aria-label={label}
      aria-current={current ? "page" : undefined}
      title={label}
      render={<a href={href} />}
    >
      {icon}
      <span className={icon ? "max-md:hidden" : undefined}>{label}</span>
    </Button>
  );
}
