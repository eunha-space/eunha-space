import { useCallback, useEffect, useRef, useState } from "react";
import { Activity, CircleAlert, HardDrive, LogOut, RefreshCw, Rocket, Server, X } from "lucide-react";
import { ApiError, api, errorMessage, type Operation, type Runtime, type Tenant } from "@/api";
import { relativeTime } from "@/format";
import { Instances } from "@/Instances";
import { Login } from "@/Login";
import { Operations } from "@/Operations";
import { Releases } from "@/Releases";
import { Tenants } from "@/Tenants";
import { pathFor, useRoute, type View } from "@/router";
import { Alert, AlertAction, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Mark, ThemeToggle } from "@/components/console/brand";
import { Spinner } from "@/components/console/status";
import { cn } from "cn";

type Session =
  | { state: "loading" }
  | { state: "signed-out" }
  | { state: "signed-in"; username: string }
  | { state: "error"; message: string };

const POLL_INTERVAL = 3000;

export function App() {
  const [session, setSession] = useState<Session>({ state: "loading" });
  const [view, navigate] = useRoute();
  const [tenants, setTenants] = useState<Tenant[]>([]);
  const [runtimes, setRuntimes] = useState<Runtime[]>([]);
  const [operations, setOperations] = useState<Operation[]>([]);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [updatedAt, setUpdatedAt] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const inFlight = useRef(false);

  const load = useCallback(async (options: { manual?: boolean } = {}) => {
    if (inFlight.current && !options.manual) return;
    inFlight.current = true;
    if (options.manual) setRefreshing(true);
    try {
      const me = await api<{ username: string }>("/session/me");
      const [nextTenants, nextRuntimes, nextOperations] = await Promise.all([
        api<Tenant[]>("/tenants"),
        api<Runtime[]>("/runtimes"),
        api<Operation[]>("/operations"),
      ]);
      setSession({ state: "signed-in", username: me.username });
      setTenants(nextTenants);
      setRuntimes(nextRuntimes);
      setOperations(nextOperations);
      setUpdatedAt(new Date().toISOString());
      setNow(Date.now());
    } catch (failure) {
      if (failure instanceof ApiError && failure.status === 401) {
        setSession({ state: "signed-out" });
        setError("");
      } else {
        const message = errorMessage(failure);
        setSession((current) => (current.state === "loading" ? { state: "error", message } : current));
        setError(message);
      }
    } finally {
      inFlight.current = false;
      setRefreshing(false);
    }
  }, []);

  /** Polling pauses while the tab is hidden and resumes the moment it is shown again. */
  useEffect(() => {
    void load();
    const timer = setInterval(() => {
      if (!document.hidden) void load();
    }, POLL_INTERVAL);
    const onVisibility = () => {
      if (!document.hidden) void load();
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [load]);

  /** Keeps "updated 20 seconds ago" honest even when polling is paused. */
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 5000);
    return () => clearInterval(timer);
  }, []);

  const act = useCallback(
    async (path: string, key: string, body?: object) => {
      setBusy(key);
      setError("");
      try {
        await api(path, { method: "POST", body: body ? JSON.stringify(body) : undefined });
        await load({ manual: true });
      } catch (failure) {
        setError(errorMessage(failure));
      } finally {
        setBusy("");
      }
    },
    [load],
  );

  if (session.state === "loading") {
    return (
      <div className="grid min-h-screen place-items-center gap-4 p-4">
        <div className="grid justify-items-center gap-3">
          <Mark />
          <p className="text-muted-foreground flex items-center gap-2 text-sm">
            <Spinner label="Loading" className="size-4" /> Loading the control plane…
          </p>
        </div>
      </div>
    );
  }

  if (session.state === "error") {
    return (
      <div className="grid min-h-screen place-items-center p-4">
        <div className="grid justify-items-center gap-3 text-center">
          <Mark />
          <h1 className="text-2xl font-semibold tracking-tight">Control plane unreachable</h1>
          <p className="text-muted-foreground max-w-prose text-sm">{session.message}</p>
          <Button onClick={() => void load({ manual: true })}>
            <RefreshCw />
            Try again
          </Button>
        </div>
      </div>
    );
  }

  if (session.state === "signed-out") {
    return <Login done={() => void load({ manual: true })} />;
  }

  async function signOut() {
    await api("/session", { method: "DELETE" });
    setSession({ state: "signed-out" });
  }

  const sections: { id: View; label: string; icon: typeof Server }[] = [
    { id: "tenants", label: "Tenants", icon: Server },
    { id: "instances", label: "Instances", icon: HardDrive },
    { id: "releases", label: "Releases", icon: Rocket },
    { id: "operations", label: "Operations", icon: Activity },
  ];
  const heading = sections.find((section) => section.id === view)?.label ?? "Tenants";

  return (
    <TooltipProvider delay={200}>
      <div className="min-h-screen md:grid md:grid-cols-[15rem_1fr]">
        <aside className="bg-sidebar border-border flex items-center gap-3 border-b p-3 md:fixed md:h-screen md:w-60 md:flex-col md:items-stretch md:gap-0 md:border-r md:border-b-0 md:p-5">
          <div className="flex items-center gap-3">
            <Mark className="size-10" />
            <div className="hidden flex-col md:flex">
              <strong className="leading-tight">eunha.space</strong>
              <span className="text-muted-foreground text-sm">Control plane</span>
            </div>
          </div>
          <nav aria-label="Console sections" className="flex gap-1 md:mt-8 md:grid">
            {sections.map((section) => (
              <Button
                key={section.id}
                variant="ghost"
                render={<a href={pathFor(section.id)} />}
                aria-current={view === section.id ? "page" : undefined}
                aria-label={section.label}
                title={section.label}
                onClick={(event) => {
                  // Let modified clicks fall through to the browser.
                  if (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
                  event.preventDefault();
                  navigate(section.id);
                }}
                className={cn(
                  // The mobile bar is icon-only; four labelled items do not fit.
                  "justify-start max-md:size-8 max-md:justify-center max-md:p-0",
                  view === section.id
                    ? "bg-sidebar-accent text-sidebar-accent-foreground font-semibold"
                    : "text-muted-foreground",
                )}
              >
                <section.icon />
                <span className="max-md:hidden">{section.label}</span>
              </Button>
            ))}
          </nav>
          <div className="ml-auto flex items-center gap-2 md:mt-auto md:ml-0 md:justify-between md:border-t md:pt-4">
            <span className="text-muted-foreground hidden truncate text-sm md:inline">
              {session.username}
            </span>
            <div className="flex items-center gap-1">
              <ThemeToggle />
              <Button
                variant="outline"
                size="icon"
                onClick={() => void signOut()}
                aria-label="Sign out"
                title="Sign out"
              >
                <LogOut />
              </Button>
            </div>
          </div>
        </aside>

        <main
          className="mx-auto w-full max-w-[92rem] p-4 md:col-start-2 md:px-8 md:py-8"
          aria-busy={refreshing}
        >
          <header className="mb-6 flex flex-wrap items-center justify-between gap-4">
            <div>
              <p className="text-primary mb-1 text-xs font-extrabold tracking-[0.18em]">RUNTIME FLEET</p>
              <h1 className="text-3xl font-semibold tracking-tight">{heading}</h1>
            </div>
            <div className="flex items-center gap-2">
              <p className="text-muted-foreground flex items-center gap-1.5 text-sm whitespace-nowrap">
                <span className="bg-primary size-1.5 animate-pulse rounded-full" aria-hidden="true" />
                {updatedAt ? `Updated ${relativeTime(updatedAt, now)}` : "Awaiting first update"}
              </p>
              <Button
                variant="outline"
                size="icon"
                onClick={() => void load({ manual: true })}
                disabled={refreshing}
                aria-label="Refresh now"
                title="Refresh now"
              >
                <RefreshCw className={refreshing ? "animate-spin" : undefined} />
              </Button>
            </div>
          </header>

          {error && (
            <Alert variant="destructive" className="mb-4">
              <CircleAlert />
              <AlertDescription>{error}</AlertDescription>
              <AlertAction>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  onClick={() => setError("")}
                  aria-label="Dismiss this message"
                >
                  <X />
                </Button>
              </AlertAction>
            </Alert>
          )}

          {view === "tenants" && (
            <Tenants tenants={tenants} runtimes={runtimes} busy={busy} act={act} />
          )}
          {view === "instances" && <Instances runtimes={runtimes} tenants={tenants} />}
          {view === "releases" && <Releases runtimes={runtimes} busy={busy} act={act} />}
          {view === "operations" && <Operations operations={operations} />}

        </main>
      </div>
    </TooltipProvider>
  );
}
