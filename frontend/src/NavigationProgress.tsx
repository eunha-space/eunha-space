import { useEffect, useState } from "react";
import { useLingui } from "@lingui/react";
import { cn } from "cn";
import { useNavigationProgress } from "@/navigation";

/** How long the bar takes to fill and fade once the page has loaded. */
const FINISH_DURATION = 300;

/**
 * A bar along the top of the window while a page loads. It creeps towards the
 * end for as long as that takes, since nothing says how long that will be.
 */
export function NavigationProgress() {
  const { i18n } = useLingui();
  const { id, active } = useNavigationProgress();
  const [shown, setShown] = useState(false);

  // Stays in the document after the page has loaded, for the bar to finish.
  useEffect(() => {
    if (active) {
      setShown(true);
      return;
    }
    const timer = window.setTimeout(() => setShown(false), FINISH_DURATION);
    return () => window.clearTimeout(timer);
  }, [active]);

  if (!active && !shown) return null;
  return (
    <div
      key={id}
      role="progressbar"
      aria-label={i18n._("Loading…")}
      className={cn(
        "bg-primary pointer-events-none fixed inset-x-0 top-0 z-50 h-[3px] origin-left transition-[scale,opacity] motion-reduce:transition-none",
        active
          ? "scale-x-90 duration-[8000ms] ease-out starting:scale-x-0"
          : "scale-x-100 opacity-0 duration-300",
      )}
    />
  );
}
