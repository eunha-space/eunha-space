const relative = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["second", 60],
  ["minute", 60],
  ["hour", 24],
  ["day", 7],
  ["week", 4.35],
  ["month", 12],
  ["year", Number.POSITIVE_INFINITY],
];

/** "just now", "4 minutes ago", "in 2 days" — paired with an absolute title attribute. */
export function relativeTime(iso: string, now: number = Date.now()): string {
  const seconds = (new Date(iso).getTime() - now) / 1000;
  if (Number.isNaN(seconds)) return "unknown";
  if (Math.abs(seconds) < 10) return "just now";
  let value = seconds;
  for (const [unit, span] of UNITS) {
    if (Math.abs(value) < span) return relative.format(Math.round(value), unit);
    value /= span;
  }
  return relative.format(Math.round(value), "year");
}

export function absoluteTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

export function clockTime(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime())
    ? iso
    : date.toLocaleTimeString(undefined, { hour12: false });
}

/** Operation kinds are stored as snake_case identifiers. */
export function humanize(value: string): string {
  const spaced = value.replaceAll("_", " ");
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

export function shortRef(ref: string | null): string {
  return ref ? ref.slice(0, 12) : "not recorded";
}

/**
 * How much of a customer's instance allowance is used, as a message to hand to
 * i18n. The counted form is built per locale because it has no catalog entry.
 */
export function instanceUsage(
  locale: string,
  used: number,
  limit: number | null | undefined,
): string {
  if (limit == null) return "No instance limit.";
  return locale === "ko"
    ? `${limit}개 중 ${used}개 사용 중입니다.`
    : `${used} of ${limit} used.`;
}
