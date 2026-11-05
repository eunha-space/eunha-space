import { useEffect, useSyncExternalStore } from "react";

/**
 * Moving between the customer pages without loading the document again, so the
 * header stays where it is. A navigation counts as under way until the requests
 * the new page makes have finished, which is what the loading bar shows.
 */

/** A request may lead to another; wait this long before calling the page loaded. */
const SETTLE_DELAY = 100;

export type Progress = {
  /** Which navigation this is, so that a new one starts the bar over. */
  id: number;
  active: boolean;
};

const listeners = new Set<() => void>();
let progress: Progress = { id: 0, active: false };
let pendingRequests = 0;
let settleTimer: number | undefined;

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

function notify() {
  listeners.forEach((listener) => listener());
}

function settleSoon() {
  window.clearTimeout(settleTimer);
  settleTimer = window.setTimeout(() => {
    if (pendingRequests > 0 || !progress.active) return;
    progress = { ...progress, active: false };
    notify();
  }, SETTLE_DELAY);
}

function startProgress() {
  progress = { id: progress.id + 1, active: true };
  settleSoon();
}

/** Holds any navigation under way open until `request` finishes. */
export function trackRequest<T>(request: Promise<T>): Promise<T> {
  pendingRequests += 1;
  return request.finally(() => {
    pendingRequests -= 1;
    settleSoon();
  });
}

function pageOf(url: Pick<Location, "pathname" | "search">) {
  return url.pathname + url.search;
}

/**
 * Goes to a customer page. `replace` is for correcting the address of the page
 * already showing, which is not a navigation the reader asked for.
 */
export function navigate(href: string, { replace = false } = {}) {
  if (replace) {
    history.replaceState(null, "", href);
    notify();
    return;
  }
  const samePage = pageOf(new URL(href, location.href)) === pageOf(location);
  if (samePage) {
    history.replaceState(null, "", href);
  } else {
    history.pushState(null, "", href);
    startProgress();
  }
  window.scrollTo(0, 0);
  notify();
}

/** The paths CustomerApp renders. Anything else on this origin needs a real load. */
function isCustomerPage(pathname: string) {
  return ["/", "/pricing", "/signin", "/signup", "/account", "/account/billing", "/instances"]
    .includes(pathname) || /^\/instances\/[^/]+$/.test(pathname);
}

function onLinkClick(event: MouseEvent) {
  if (
    event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey ||
    event.shiftKey || event.altKey || !(event.target instanceof Element)
  ) return;
  const link = event.target.closest("a");
  if (!link || !link.href || link.hasAttribute("download")) return;
  if (link.target && link.target !== "_self") return;
  const url = new URL(link.href);
  if (url.origin !== location.origin || !isCustomerPage(url.pathname)) return;
  // The browser already scrolls to an anchor without loading anything.
  if (url.hash && pageOf(url) === pageOf(location)) return;
  event.preventDefault();
  navigate(url.pathname + url.search + url.hash);
}

let shownPage = pageOf(location);

function onPopState() {
  // Going back to an anchor on the same page is not a navigation.
  if (pageOf(location) !== shownPage) startProgress();
  notify();
}

/**
 * The page to render, re-rendering the caller when it changes. Links anywhere
 * in the document go through `navigate` while this is mounted.
 */
export function useCustomerNavigation(): string {
  useEffect(() => {
    document.addEventListener("click", onLinkClick);
    window.addEventListener("popstate", onPopState);
    return () => {
      document.removeEventListener("click", onLinkClick);
      window.removeEventListener("popstate", onPopState);
    };
  }, []);
  shownPage = useSyncExternalStore(subscribe, () => pageOf(location));
  return shownPage;
}

export function useNavigationProgress(): Progress {
  return useSyncExternalStore(subscribe, () => progress);
}
