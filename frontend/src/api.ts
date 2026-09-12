export type TenantState = "healthy" | "unhealthy" | "disabled" | "unknown";

export type Tenant = {
  id: string;
  domain: string;
  title: string;
  desired_state: string;
  observed_state: TenantState;
  config_revision: number;
  rendered_revision: number | null;
  runtime_name: string;
  bind_address: string;
  updated_at: string;
};

export type Operation = {
  id: string;
  domain: string | null;
  runtime_name: string | null;
  kind: string;
  status: string;
  error: string | null;
  created_at: string;
};

export type Runtime = {
  id: string;
  name: string;
  bind_address: string;
  auto_deploy_enabled: boolean;
  deployed_ref: string | null;
  release_operation_id: string | null;
  release_status: string | null;
  release_message: string | null;
};

export type OperationEvent = {
  id: number;
  message: string;
  created_at: string;
};

/** Thrown for any non-2xx API response; `status` lets callers detect a lost session. */
export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`/api${path}`, {
    ...init,
    headers: { "Content-Type": "application/json", ...(init?.headers || {}) },
  });
  if (!response.ok) {
    const body = await response.json().catch(() => ({}) as { error?: string });
    throw new ApiError(body.error || `Request failed (${response.status})`, response.status);
  }
  return response.status === 204 ? (undefined as T) : response.json();
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** An operation that has finished has no further events to poll for. */
export function isSettled(status: string | null): boolean {
  return status === "succeeded" || status === "failed";
}

export function pendingRender(tenant: Tenant): boolean {
  return tenant.rendered_revision !== tenant.config_revision;
}
