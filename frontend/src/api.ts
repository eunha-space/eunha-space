export type TenantState = "healthy" | "unhealthy" | "disabled" | "unknown";

export type Tenant = {
  id: string;
  domain: string;
  title: string;
  lifecycle_state:
    | "provisioning"
    | "ready"
    | "failed"
    | "deprovisioning"
    | "retained"
    | "purged";
  desired_state: string;
  observed_state: TenantState;
  config_revision: number;
  rendered_revision: number | null;
  runtime_name: string;
  bind_address: string;
  updated_at: string;
};

export type CustomerDomainSetup = {
  hostname: string;
  kind: "provided" | "custom";
  status: "active" | "pending" | "failed" | "removing";
  is_primary: boolean;
  cname_target: string;
  ownership_name: string | null;
  ownership_value: string | null;
  ssl_status: string | null;
};

export type CustomerProvisioningStep = {
  step:
    | "database"
    | "migrate"
    | "render"
    | "reload"
    | "health"
    | "routing"
    | "owner";
  status: "pending" | "running" | "succeeded" | "failed";
  updated_at: string;
};

export type CustomerTenantDetail = {
  tenant: Pick<
    Tenant,
    | "id"
    | "domain"
    | "title"
    | "lifecycle_state"
    | "observed_state"
    | "updated_at"
  > & {
    /** The instance's Owner account; null for instances created without one. */
    owner_username: string | null;
    owner_ready: boolean;
    /** When the customer area last issued a password for the Owner. */
    owner_password_issued_at: string | null;
  };
  domains: CustomerDomainSetup[];
  provisioning_steps: CustomerProvisioningStep[];
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
  /** Stable operator-facing name of the physical instance. */
  instance_name: string;
  /** Address of the instance the runtime runs on, from the instances table. */
  instance_address: string;
  bind_address: string;
  service_label: string;
  tenant_directory: string;
  origin_hostname: string;
  router_enabled: boolean;
  blue_bind_address: string;
  green_bind_address: string;
  tenant_count: number;
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
    throw new ApiError(
      body.error || `Request failed (${response.status})`,
      response.status,
    );
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

export type TenantConfiguration = {
  domain: string;
  file_name: string;
  config_revision: number;
  rendered_revision: number | null;
  /** Rendered exactly as a reconciliation would write it, minus every secret. */
  toml: string;
};

export type TenantDomain = {
  id: string;
  hostname: string;
  kind: "provided" | "custom" | "legacy";
  status: "active" | "pending" | "failed" | "removing";
  is_primary: boolean;
  cloudflare_status: string | null;
  ssl_status: string | null;
  cname_target: string;
  ownership_name: string | null;
  ownership_value: string | null;
  error: string | null;
};
