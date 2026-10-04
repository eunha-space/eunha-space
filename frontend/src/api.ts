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
    | "purging"
    | "purged";
  desired_state: string;
  observed_state: TenantState;
  config_revision: number;
  rendered_revision: number | null;
  runtime_name: string;
  bind_address: string;
  updated_at: string;
  /** When a deleted instance's retained data is purged, unless it is restored first. */
  purge_scheduled_for: string | null;
  /** A custom domain released because it never validated; `domain` is then a placeholder. */
  abandoned_custom_domain: string | null;
  /**
   * An existing instance that was taken over rather than created. Its database,
   * media and accounts predate the platform, which is why a purge refuses to
   * destroy the parts of it that were never ours.
   */
  imported: boolean;
  /** Durable Temporal execution metadata; no workflow payloads or secrets are exposed. */
  temporal_workflow_id: string | null;
  /** The operator-only Temporal UI link for that execution, when one is configured. */
  temporal_workflow_url: string | null;
  temporal_workflow_kind: "provision" | "deprovision" | "purge" | null;
  temporal_dispatch_attempts: number | null;
  lifecycle_step: string | null;
};

/** The address to show for an instance, including one whose domain was released. */
export function instanceAddress(
  tenant: Pick<Tenant, "domain" | "abandoned_custom_domain">,
): string {
  return tenant.abandoned_custom_domain ?? tenant.domain;
}

/** Whether a tenant is being or has been taken offline; the runtime does not serve it. */
export function isOffline(tenant: Pick<Tenant, "lifecycle_state">): boolean {
  return ["deprovisioning", "retained", "purging", "purged"].includes(
    tenant.lifecycle_state,
  );
}

/** A deprovisioned instance keeps its data but frees its slot. */
export function countsTowardInstanceLimit(tenant: Pick<Tenant, "lifecycle_state">) {
  return !isOffline(tenant);
}

/** The customer-facing name of an instance's state, as an untranslated message. */
export function instanceStatus(
  tenant: Pick<Tenant, "lifecycle_state">,
): string {
  switch (tenant.lifecycle_state) {
    case "ready":
      return "Ready";
    case "failed":
      return "Needs attention";
    case "deprovisioning":
      return "Deleting";
    case "retained":
    case "purged":
      return "Deleted";
    default:
      return "Setting up";
  }
}

export type CustomerDomainSetup = {
  hostname: string;
  kind: "provided" | "custom";
  status: "active" | "pending" | "failed" | "removing";
  is_primary: boolean;
  cname_target: string;
  ownership_name: string | null;
  ownership_value: string | null;
  ssl_status: string | null;
  /** Why the last check did not connect it. */
  check_problem: "dns" | "retrying" | null;
  checked_at: string | null;
  /** When setup stops waiting for this domain and continues without it. */
  setup_deadline: string | null;
};

export type CustomerProvisioningStep = {
  step:
    | "database"
    | "migrate"
    | "import"
    | "render"
    | "media"
    | "reload"
    | "health"
    | "routing"
    | "owner";
  status: "pending" | "running" | "succeeded" | "failed";
  updated_at: string;
};

/** The signed-in customer, as /customer/session/me reports them. */
export type Customer = {
  is_admin: boolean;
  email: string;
  instance_limit: number | null;
  has_password: boolean;
  // False in a session established by an email link, which is itself the
  // proof a forgotten password needs.
  requires_current_password: boolean;
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
    | "purge_scheduled_for"
    | "abandoned_custom_domain"
  > & {
    /** The instance's Owner account; null for instances created without one. */
    owner_username: string | null;
    /** What the owner signs in to Eunha with. */
    owner_email: string | null;
    owner_ready: boolean;
    /** When the customer area last issued a password for the Owner. */
    owner_password_issued_at: string | null;
  };
  domains: CustomerDomainSetup[];
  provisioning_steps: CustomerProvisioningStep[];
  /** The instance's latest domain change, running or finished. */
  domain_change: DomainChange | null;
};

/** Moving a served instance to another domain. */
export type DomainChange = {
  id: string;
  from_domain: string;
  to_domain: string;
  status: "running" | "succeeded" | "failed" | "cancelled";
  step: "attach" | "rename" | "switch" | "move" | "deliver" | "release" | "done";
  /** Deliveries of the Moves not yet through or given up on. */
  pending: number | null;
  /** Followers' servers the Move was given up on for. */
  given_up: number | null;
  error: string | null;
  created_at: string;
  finished_at: string | null;
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
  instance_id: string;
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
  // Accepted work (202) and No Content (204) may both come back without a body.
  const text = await response.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** An operation that has finished has no further events to poll for. */
export function isSettled(status: string | null): boolean {
  return status === "succeeded" || status === "failed";
}

export function pendingRender(tenant: Tenant): boolean {
  // A tenant taken offline is deliberately not rendered.
  return !isOffline(tenant) && tenant.rendered_revision !== tenant.config_revision;
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
