import { useCallback, useEffect, useMemo, useState } from "react";
import { CircleAlert, LogIn, UsersRound } from "lucide-react";
import { api, errorMessage } from "@/api";
import { absoluteTime, humanize, relativeTime } from "@/format";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Card, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { ActionButton } from "@/components/console/action-button";
import { ChipFilter, SearchField } from "@/components/console/filters";
import { EmptyState, Spinner } from "@/components/console/status";

/** An account, as /users lists it for the console. */
type User = {
  id: string;
  email: string;
  is_admin: boolean;
  verified: boolean;
  created_at: string;
  instances: number;
  /** Null for an account that never subscribed. */
  subscription: string | null;
  /** Null when no session of their own is still kept (30 days). */
  last_signed_in_at: string | null;
};

type Filter = "all" | "customers" | "admins" | "unverified";

function matches(user: User, filter: Filter): boolean {
  switch (filter) {
    case "all":
      return true;
    case "customers":
      return !user.is_admin;
    case "admins":
      return user.is_admin;
    case "unverified":
      return !user.verified;
  }
}

export function Users() {
  const [users, setUsers] = useState<User[] | null>(null);
  const [error, setError] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState("");

  const load = useCallback(async () => {
    try {
      setUsers(await api<User[]>("/users"));
      setError("");
    } catch (failure) {
      setError(errorMessage(failure));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const counts = useMemo(() => {
    const tally = {} as Record<Filter, number>;
    for (const key of ["all", "customers", "admins", "unverified"] as Filter[]) {
      tally[key] = (users ?? []).filter((user) => matches(user, key)).length;
    }
    return tally;
  }, [users]);
  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return (users ?? []).filter((user) =>
      matches(user, filter) && (needle === "" || user.email.includes(needle))
    );
  }, [users, filter, query]);

  async function signInAs(user: User) {
    setBusy(user.id);
    setError("");
    try {
      await api(`/users/${user.id}/masquerade`, { method: "POST" });
      location.assign("/instances");
    } catch (failure) {
      setError(errorMessage(failure));
      setBusy("");
    }
  }

  if (!users) {
    return error
      ? (
        <Alert variant="destructive">
          <CircleAlert />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )
      : <Spinner label="Loading users" className="size-5" />;
  }

  return (
    <div className="grid gap-4">
      {error && (
        <Alert variant="destructive">
          <CircleAlert />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      <Card className="gap-0 overflow-hidden p-0">
        <CardHeader className="flex flex-col gap-3 px-4 py-3">
          <div>
            <CardTitle className="text-base">Users</CardTitle>
            <CardDescription className="text-xs">
              Every account that can sign in, newest first. Their subscriptions and payments are
              under Billing.
            </CardDescription>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <SearchField
              value={query}
              onChange={setQuery}
              label="Search users"
              placeholder="Search email"
            />
            <ChipFilter
              label="Filter users"
              value={filter}
              onChange={(value) => setFilter(value as Filter)}
              choices={[
                { value: "all", label: "All", count: counts.all },
                { value: "customers", label: "Customers", count: counts.customers },
                { value: "admins", label: "Administrators", count: counts.admins },
                { value: "unverified", label: "Unverified", count: counts.unverified },
              ]}
            />
          </div>
        </CardHeader>
        <Separator />
        {visible.length === 0
          ? (
            <EmptyState icon={<UsersRound />}>
              {users.length === 0
                ? "Nobody has signed up yet."
                : "No users match the current search and filter."}
            </EmptyState>
          )
          : (
            <div className="overflow-x-auto">
              <Table>
                <TableCaption className="sr-only">User accounts</TableCaption>
                <TableHeader>
                  <TableRow>
                    <TableHead>Account</TableHead>
                    <TableHead className="hidden md:table-cell">Subscription</TableHead>
                    <TableHead className="text-right">Instances</TableHead>
                    <TableHead className="hidden md:table-cell">Joined</TableHead>
                    <TableHead className="hidden lg:table-cell">Last signed in</TableHead>
                    <TableHead>
                      <span className="sr-only">Actions</span>
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {visible.map((user) => (
                    <TableRow key={user.id}>
                      <TableCell>
                        <span className="flex flex-wrap items-center gap-2">
                          {user.email}
                          {user.is_admin && <Badge variant="outline">admin</Badge>}
                          {!user.verified && (
                            <Badge variant="outline" className="text-muted-foreground">
                              unverified
                            </Badge>
                          )}
                        </span>
                      </TableCell>
                      <TableCell className="text-muted-foreground hidden md:table-cell">
                        {user.subscription ? humanize(user.subscription) : "—"}
                      </TableCell>
                      <TableCell className="text-right tabular-nums">{user.instances}</TableCell>
                      <TableCell
                        className="text-muted-foreground hidden md:table-cell"
                        title={absoluteTime(user.created_at)}
                      >
                        {relativeTime(user.created_at)}
                      </TableCell>
                      <TableCell
                        className="text-muted-foreground hidden lg:table-cell"
                        title={user.last_signed_in_at
                          ? absoluteTime(user.last_signed_in_at)
                          : undefined}
                      >
                        {user.last_signed_in_at ? relativeTime(user.last_signed_in_at) : "—"}
                      </TableCell>
                      <TableCell className="text-right">
                        {!user.is_admin && (
                          <ActionButton
                            icon={<LogIn />}
                            iconOnly
                            variant="ghost"
                            busy={busy === user.id}
                            disabled={!user.verified}
                            disabledReason="Their email is not verified yet."
                            onClick={() => void signInAs(user)}
                            confirm={{
                              title: `Sign in as ${user.email}?`,
                              description:
                                "You see their instances and can act on them for an hour, or until you come back. Their password, billing and language stay theirs. The operations log records that you signed in as them.",
                              action: "Sign in as them",
                            }}
                          >
                            Sign in as {user.email}
                          </ActionButton>
                        )}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          )}
      </Card>
    </div>
  );
}
