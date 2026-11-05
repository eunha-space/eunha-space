import { useLingui } from "@lingui/react";
import { siteColumn } from "@/components/customer/page";
import { cn } from "cn";

// The seller's details Korean e-commerce law requires on the site. They are
// stated as registered, so they stay in Korean whatever the interface language.
const BUSINESS_DETAILS = [
  ["상호명", "화양전자"],
  ["사업자등록번호", "101-28-99756"],
  ["통신판매업신고번호", "2026-서울성동-1013"],
  ["대표자명", "서지혁"],
  ["사업장 주소", "서울 성동구 연무장길 31 101동 1706호"],
  ["유선번호", "010-5828-3026"],
] as const;

export function SiteFooter() {
  const { i18n } = useLingui();
  const t = (message: string) => i18n._(message);
  const links = [
    { href: "/pricing", label: t("Pricing") },
    { href: "/refund-policy", label: t("Refund policy") },
    { href: "https://eunha.social", label: t("About Eunha") },
    { href: "https://github.com/eunha-space/eunha", label: t("Source code") },
  ];
  return (
    <footer className="text-muted-foreground border-t py-6 text-xs">
      <div className={cn(siteColumn, "grid gap-4")}>
        <nav aria-label={t("Site")} className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
          {links.map(({ href, label }) => (
            <a key={href} href={href} className="hover:text-foreground underline-offset-4 hover:underline">
              {label}
            </a>
          ))}
        </nav>
        <dl lang="ko" className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
          {BUSINESS_DETAILS.map(([label, value]) => (
            <div key={label} className="col-span-2 grid grid-cols-subgrid">
              <dt>{label}</dt>
              <dd className="text-foreground">{value}</dd>
            </div>
          ))}
        </dl>
      </div>
    </footer>
  );
}
