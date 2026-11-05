import { useLingui } from "@lingui/react";
import { LogOut, Server, Shield, UserRound } from "lucide-react";
import type { Customer } from "@/api";
import { Button } from "@/components/ui/button";
import { Mark, ThemeToggle } from "@/components/console/brand";
import { LanguageToggle } from "@/i18n";

/**
 * The top of every page but the console. `customer` is undefined until the
 * session is known, when the header offers neither signing in nor an account.
 */
export function SiteHeader(
  { customer, onSignOut }: {
    customer: Customer | null | undefined;
    onSignOut: () => Promise<void>;
  },
) {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  return (
    <div className="mx-auto w-full max-w-5xl px-4 md:px-8">
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
        <div className="flex shrink-0 gap-2">
          {/* There is no room on a phone; the landing page links here further down. */}
          <Button variant="ghost" className="hidden sm:inline-flex" render={<a href="/pricing" />}>
            {t("Pricing")}
          </Button>
          {customer?.is_admin && (
            <Button
              variant="ghost"
              className="max-sm:aspect-square max-sm:px-0"
              aria-label={t("Control plane")}
              render={<a href="/admin" />}
            >
              <Shield className="sm:hidden" />
              <span className="max-sm:hidden">{t("Control plane")}</span>
            </Button>
          )}
          <LanguageToggle />
          <ThemeToggle />
          {customer === null && (
            <Button variant="outline" render={<a href="/signin" />}>
              {t("Sign in")}
            </Button>
          )}
          {customer && (
            <>
              <Button
                variant="outline"
                className="max-sm:aspect-square max-sm:px-0"
                aria-label={t("Your instances")}
                render={<a href="/instances" />}
              >
                <Server />
                <span className="max-sm:hidden">{t("Your instances")}</span>
              </Button>
              <Button
                variant="outline"
                size="icon"
                aria-label={t("Account")}
                title={customer.email}
                render={<a href="/account" />}
              >
                <UserRound />
              </Button>
              <Button
                variant="outline"
                size="icon"
                aria-label={t("Sign out")}
                title={t("Sign out")}
                onClick={() => void onSignOut()}
              >
                <LogOut />
              </Button>
            </>
          )}
        </div>
      </header>
    </div>
  );
}
