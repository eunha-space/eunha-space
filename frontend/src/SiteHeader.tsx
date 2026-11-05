import { useLingui } from "@lingui/react";
import { Button } from "@/components/ui/button";
import { Mark, ThemeToggle } from "@/components/console/brand";
import { LanguageToggle } from "@/i18n";

/** The top of the pages anyone can read. */
export function SiteHeader() {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  return (
    <header className="flex items-center justify-between gap-3 py-5">
      <a href="/" className="flex min-w-0 items-center gap-2 font-semibold">
        <Mark className="size-9 shrink-0" />
        {t("eunha space")}
      </a>
      <div className="flex shrink-0 gap-2">
        {/* There is no room on a phone; the landing page links here further down. */}
        <Button variant="ghost" className="hidden sm:inline-flex" render={<a href="/pricing" />}>
          {t("Pricing")}
        </Button>
        <LanguageToggle />
        <ThemeToggle />
        <Button variant="outline" render={<a href="/signin" />}>
          {t("Sign in")}
        </Button>
      </div>
    </header>
  );
}
