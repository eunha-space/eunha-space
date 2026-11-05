import { useLingui } from "@lingui/react";
import { instanceStatus, type Tenant } from "@/api";
import { StateBadge } from "@/components/customer/page";

const TONE = {
  Ready: "ok",
  "Setting up": "waiting",
  Deleting: "waiting",
  "Needs attention": "problem",
  Deleted: "neutral",
} as const;

/** An instance's state, the same in the list and on its own page. */
export function InstanceStatusBadge(
  { tenant }: { tenant: Pick<Tenant, "lifecycle_state"> },
) {
  const { i18n } = useLingui();
  const status = instanceStatus(tenant);
  return <StateBadge tone={TONE[status as keyof typeof TONE] ?? "waiting"}>{i18n._(status)}</StateBadge>;
}
