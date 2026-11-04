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
  return (
    <footer lang="ko" className="text-muted-foreground border-t px-4 py-6 text-xs">
      <dl className="mx-auto grid max-w-2xl grid-cols-[auto_1fr] gap-x-4 gap-y-1">
        {BUSINESS_DETAILS.map(([label, value]) => (
          <div key={label} className="col-span-2 grid grid-cols-subgrid">
            <dt>{label}</dt>
            <dd className="text-foreground">{value}</dd>
          </div>
        ))}
      </dl>
    </footer>
  );
}
