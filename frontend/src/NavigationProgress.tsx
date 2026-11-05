import { useEffect, useLayoutEffect, useRef, useState } from "react";
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
  return <Bar key={id} active={active} />;
}

function Bar({ active }: { active: boolean }) {
  const { i18n } = useLingui();
  const bar = useRef<HTMLDivElement>(null);
  const [started, setStarted] = useState(false);

  // Not @starting-style: Safari ignores it when set through Tailwind's scale
  // variables, and the bar then starts full and shrinks. Reading the layout has
  // the browser take the empty bar as where the transition begins.
  useLayoutEffect(() => {
    bar.current?.getBoundingClientRect();
    setStarted(true);
  }, []);

  return (
    <div
      ref={bar}
      role="progressbar"
      aria-label={i18n._("Loading…")}
      style={{ scale: active ? (started ? "0.9 1" : "0 1") : "1 1" }}
      className={cn(
        "bg-primary pointer-events-none fixed inset-x-0 top-0 z-50 h-[3px] origin-left transition-[scale,opacity] motion-reduce:transition-none",
        active ? "duration-[8000ms] ease-out" : "opacity-0 duration-300",
      )}
    />
  );
}
