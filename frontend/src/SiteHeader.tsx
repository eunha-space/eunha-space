import { useLingui } from "@lingui/react";
import { Eye, LogOut, Server, Shield, UserRound } from "lucide-react";
import type { Customer } from "@/api";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Mark, ThemeToggle } from "@/components/console/brand";
import { siteColumn } from "@/components/customer/page";
import { LanguageToggle } from "@/i18n";
import { cn } from "cn";

/**
 * The top of every page but the console: where to go, then the site's settings,
 * then the account and its session. `customer` is undefined until the session
 * is known, when the header offers neither signing in nor an account.
 */
export function SiteHeader(
  { customer, onSignOut }: {
    customer: Customer | null | undefined;
    onSignOut: () => Promise<void>;
  },
) {
  const { i18n } = useLingui();
  const t = (message: string, values?: Record<string, string>) => i18n._(message, values);
  const path = location.pathname;
  return (
    <div className={siteColumn}>
      {customer?.masquerading_admin && (
        <Alert className="mt-4 flex flex-wrap items-center justify-between gap-2">
          <AlertDescription className="flex items-center gap-2">
            <Eye className="size-4 shrink-0" />
            {t("You are signed in as {email}.", { email: customer.email })}
          </AlertDescription>
          <Button size="sm" variant="outline" onClick={() => void onSignOut()}>
            {t("Back to the control plane")}
          </Button>
        </Alert>
      )}
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
              <NavLink
                href="/instances"
                label={t("Your instances")}
                icon={<Server />}
                current={path === "/instances" || path.startsWith("/instances/")}
              />
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
            <AccountMenu
              email={customer.email}
              current={path === "/account" || path === "/account/billing"}
              onSignOut={onSignOut}
            />
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

/** The account page and signing out, behind the account's button. */
function AccountMenu(
  { email, current, onSignOut }: {
    email: string;
    current: boolean;
    onSignOut: () => Promise<void>;
  },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button
            variant="outline"
            className={cn("max-md:aspect-square max-md:px-0", current && "bg-muted")}
            aria-label={t("Account")}
            title={email}
          />
        }
      >
        <UserRound />
        <span className="max-md:hidden">{t("Account")}</span>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-56">
        <DropdownMenuGroup>
          <DropdownMenuLabel className="truncate">{email}</DropdownMenuLabel>
          {/* A plain link, so that it changes the page in place like the others. */}
          <DropdownMenuItem render={<a href="/account" />}>
            <UserRound /> {t("Account")}
          </DropdownMenuItem>
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuItem onClick={() => void onSignOut()}>
          <LogOut /> {t("Sign out")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
