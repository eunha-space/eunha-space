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
  const [root, section = ""] = pathname.split("/").filter(Boolean);
  return `/${root}` === BASE_PATH && (SECTIONS as readonly string[]).includes(section) ? (section as View) : null;
}

/** Segments below a section, e.g. ["tenants", id] for /admin/usage/tenants/<id>. */
export function subpathFromPath(view: View, pathname: string): string[] {
  if (viewFromPath(pathname) !== view) return [];
  return pathname.split("/").filter(Boolean).slice(2).map(segment => {
    try { return decodeURIComponent(segment); } catch { return segment; }
  });
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
    // Subroutes listen for popstate too; returning to a section's root resets them.
    window.dispatchEvent(new PopStateEvent("popstate"));
  }, []);

  return [view, navigate];
}

/** A section's own pages, kept in the path below the section. */
export function useSubroute(view: View): [string[], (segments: string[]) => void] {
  const [segments, setSegments] = useState(() => subpathFromPath(view, location.pathname));

  useEffect(() => {
    const onPopState = () => setSegments(subpathFromPath(view, location.pathname));
    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, [view]);

  const navigate = useCallback((next: string[]) => {
    history.pushState(null, "", [pathFor(view), ...next.map(encodeURIComponent)].join("/"));
    setSegments(next);
  }, [view]);

  return [segments, navigate];
}
