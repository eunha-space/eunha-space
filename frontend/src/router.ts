import { useCallback, useEffect, useState } from "react";

/** The console lives under /admin, one path per section. */
export const BASE_PATH = "/admin";

export const SECTIONS = ["tenants", "usage", "instances", "releases", "operations"] as const;
export type View = (typeof SECTIONS)[number];

const DEFAULT_VIEW: View = "tenants";

export function pathFor(view: View): string {
  return `${BASE_PATH}/${view}`;
}

export function viewFromPath(pathname: string): View | null {
  const last = pathname.replace(/\/+$/, "").split("/").pop() ?? "";
  return (SECTIONS as readonly string[]).includes(last) ? (last as View) : null;
}

/**
 * Keeps the visible section in the URL so a deep link and a refresh both land
 * where the operator was. The server serves index.html for unmatched paths, so
 * every section URL resolves on a cold load.
 */
export function useRoute(): [View, (view: View) => void] {
  const [view, setView] = useState<View>(() => viewFromPath(location.pathname) ?? DEFAULT_VIEW);

  useEffect(() => {
    // Canonicalise "/" or an unknown path without leaving a history entry.
    if (viewFromPath(location.pathname) === null) {
      history.replaceState(null, "", pathFor(viewFromPath(location.pathname) ?? DEFAULT_VIEW));
    }
    const onPopState = () => setView(viewFromPath(location.pathname) ?? DEFAULT_VIEW);
    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, []);

  const navigate = useCallback((next: View) => {
    history.pushState(null, "", pathFor(next));
    setView(next);
  }, []);

  return [view, navigate];
}
