import type { PublicPlan } from "@/api";

/** Where to ask about a plan bigger than those on offer. */
export const CONTACT_EMAIL = "hello@eunha.space";

/** What a plan promises beyond its price, in the reader's language. */
export function planLimits(plan: PublicPlan, korean: boolean): string[] {
  const compact = (value: number) =>
    new Intl.NumberFormat(korean ? "ko-KR" : "en-US", { notation: "compact" }).format(value);
  const limits: (string | null)[] = korean
    ? [
      plan.instance_limit == null ? null : `인스턴스 ${plan.instance_limit}개`,
      plan.user_limit == null ? null : `사용자 ${plan.user_limit}명`,
      plan.media_gb == null ? null : `미디어 ${plan.media_gb}GB`,
      plan.database_gb == null ? null : `데이터베이스 ${plan.database_gb}GB`,
      plan.monthly_activities == null
        ? null
        : `월 ActivityPub 활동 ${compact(plan.monthly_activities)}건 (공정 사용 기준)`,
    ]
    : [
      plan.instance_limit == null
        ? null
        : `${plan.instance_limit} instance${plan.instance_limit === 1 ? "" : "s"}`,
      plan.user_limit == null ? null : `${plan.user_limit} users`,
      plan.media_gb == null ? null : `${plan.media_gb} GB media`,
      plan.database_gb == null ? null : `${plan.database_gb} GB database`,
      plan.monthly_activities == null
        ? null
        : `${compact(plan.monthly_activities)} ActivityPub activities a month (fair use)`,
    ];
  return limits.filter((limit) => limit !== null);
}
