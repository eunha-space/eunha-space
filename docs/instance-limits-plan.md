# Plan: measuring instances and enforcing plan limits

Status: Phase 1 implemented, 2026-11-07. Phases 2–4 remain gated on two to
four weeks of measurements and a review of the limits and pricing.

Implementation review
---------------------

- The capacity sampler runs independently of the ten-second PostgreSQL health
  probe. It checks for due tenants each minute and collects at most once per
  fifteen minutes, with eight probes in flight and a four-second tenant budget.
  Users exclude suspended accounts as well as unconfirmed, unapproved and
  disabled users. Media sums cast each size to bigint before adding.
- Raw capacity samples live for 28 days. Daily capacity readings retain the
  latest attempt of each UTC day for 365 days; a failed latest attempt stays
  missing. These are end-of-day snapshots, not daily maxima or averages.
- Media allowances are displayed in decimal GB of recorded originals,
  thumbnails, local avatars, headers and emoji. Remote media is separate.
  Customer-owned imported storage is marked excluded from the allowance.
  Bucket listing and calibration remain an operator follow-up; no storage
  multiplier is guessed.
- Ojak's optional `metrics` feature adds `Deliverer::metrics(labels)`, using
  the application's recorder alongside its existing callbacks. Its bounded
  outcome dimension separates successful sends, failures and breaker holds.
  Eunha supplies the canonical tenant label. No sender, inbox, activity ID,
  error or payload becomes a metric label.
- Activities mean **successful outbound sends**, not unique activities or
  HTTP attempts. Queue acknowledgement failure can lead to a counted
  redelivery. Failed and held attempts are exported separately.
- Calendar months use UTC. The scraper persists counter deltas atomically
  into daily delivery rollups, rejecting duplicate or out-of-order scrapes.
  Deltas require the same process epoch, monotonic counters, and at most a
  thirty-second interval within one UTC day. Restarts, gaps and midnight
  intervals are unknown. The UI labels totals as observed lower bounds and
  shows covered hours, never presenting partial collection as exact billing.
  These totals remain fair-use observations only.
- Admin fleet and tenant capacity tables compare current readings with plan
  limits, keep unavailable/stale readings visible, and show daily history on
  the tenant page. Retained tenants remain operator-only storage inventory.

Decisions before Phases 2–4
---------------------------

Keep the proposed sign-in-capable account count, rather than changing a
promised user limit to a moving 30-day activity count. Keep database and
activities advisory: neither has a safe growth-only blocking rule. A
14-day grace period is a reasonable proposal, but no grace or enforcement
state is activated in Phase 1. Remote cache caps require a separate runtime
retention policy. After the observation window, confirm these choices and
calibrate recorded media sizes against actual storage before member warnings
or growth restrictions are enabled.

Plans promise five limits (`plans.instance_limit`, `user_limit`, `media_gb`,
`database_gb`, `monthly_activities`). Only the instance count is enforced
today. This plan measures the other four per instance, shows them, and then
enforces them. Measurement comes first and runs on its own for a few weeks.
That way the limits, and the prices, are set from real numbers rather than
guesses.

## Principles

- **Over a limit blocks growth; it never stops an instance.** An instance over
  its plan keeps serving its members and federating. What stops is whatever
  makes it bigger: new sign-ups, new uploads. Stopping a community's server
  over a quota punishes its members for the owner's billing. It would also turn
  a refund or a downgrade into an outage. With this rule, a refund to a
  smaller plan only needs a warning, not a refusal (the instance-count refusal
  stays, because a count can't be "blocked from growing" once it is over).
- **Measure what the member can act on.** Each number must be something the
  owner can bring down themselves: delete media, remove users, prune remote
  caches.
- **Never guess.** A metric that failed to collect is missing, not zero, as
  `/admin/usage` already does. Enforcement never acts on a missing or stale
  reading.
- **Warn before enforcing.** At 80% the owner gets a warning. At 100% they get
  a grace period. Only then is growth blocked.

## What each limit measures

| Limit | Measured as | Source | Cost |
| --- | --- | --- | --- |
| Users | Local accounts that can sign in: `users` joined to `accounts` with `domain IS NULL`, confirmed, approved, not disabled, the instance actor excluded | Tenant database, SQL | Cheap |
| Media | Bytes of **local** media: `media_attachments` with no `remote_url` (file plus thumbnail sizes), local accounts' avatars and headers, local custom emoji | Tenant database, SQL (as `tootctl media usage` does) | Cheap, indexed scans |
| Database | `pg_database_size` | Already collected by `/admin/usage` | Done |
| Activities | Outbound ActivityPub deliveries per calendar month | New tenant-labelled counter in the runtime's `/metrics` | Small Eunha change |

Points to settle while building:

- **Remote media cache is not counted.** It is ours to bound: Eunha already
  removes it on a schedule (`eunha media remove`). Counting it would charge
  members for other servers' posts. Its total is still shown to operators,
  because it costs storage.
- **Media sizes from the database understate the bucket.** `file_file_size`
  is the original's size; Mastodon's resized variants are not recorded.
  Measure the gap once against R2 for a few tenants (list each tenant's key
  prefix), and either state the limit as "originals" or apply a measured
  factor. Don't list buckets on every collection round.
- **Imported instances whose media lives in their own bucket** (an
  `ImportedMediaStorage`) don't use our storage. Exclude their media from the
  limit and say so on their page.
- **Retained databases** still use storage but have no subscription. Show them
  to operators only.
- **Activities stay fair use** (as the pricing page already says). They feed
  operator alerts, never automatic blocking. A post that goes viral on another
  server isn't the owner's doing.

## Phase 1: collect (control plane only)

1. Extend the database probe in `src/usage.rs` to read the user count and
   local media bytes, with the existing per-probe timeout. These change
   slowly, so read them every 15 minutes rather than every 10 seconds.
2. Store the readings in a new table: `tenant_usage(tenant_id, sampled_at,
   users, local_media_bytes, remote_media_bytes, database_bytes)`. Also keep a
   daily rollup, retained for a year: billing questions ("was I over last
   month?") outlive the 28-day sample window.
3. Eunha: add a tenant-labelled counter for outbound deliveries, at the point
   the delivery job finishes, and read it with the existing `/metrics`
   scraper. The counting hook is generic ActivityPub, so it belongs in ojak
   (labelled by Eunha). Add a monthly total to the daily rollup.
4. Show the readings on `/admin/usage/tenants/<id>` and add a fleet table:
   each tenant's usage against its plan, sorted by percentage of plan used.

Exit: two to four weeks of data. With it:
- check the limits against real instances,
- answer the earlier "what does an instance cost us" question (storage per GB
  is known, and database and media now are too),
- revisit plan pricing before anything is enforced.

## Phase 2: show it to members

1. Each instance on the account page shows a meter per limit ("Users 34 / 50,
   Media 3.1 GB / 5 GB"), with when it was last measured.
2. The billing view's plan change and refund previews gain `over_limits`: the
   limits the new plan would be under, per instance, with the numbers. The
   refund dialog's alert (built today for the instance count) and the plan
   change dialog show it. Under the growth-only rule this is a warning; the
   member can still go ahead.
3. A scheduled downgrade that would be over at renewal is warned about when
   it is scheduled, and emailed again three days before it applies.

## Phase 3: warn

1. A daily job compares the latest reading with the plan, per instance.
   - At 80%, email the owner once per limit per period.
   - At 100%, email the owner and start a grace period (proposed: 14 days),
     recorded in `tenant_limit_states(tenant_id, limit, over_since, notified_at,
     enforced_at)`.
2. A Billing admin page lists instances over or near their limits, with grace
   dates. Operators can extend a grace period or exempt an instance (recorded,
   like other billing operations).
3. Coming back under clears the state, and the owner is told.

## Phase 4: enforce (blocking growth only)

Enforcement lives in Eunha, the runtime, because that is where sign-ups and
uploads happen. The control plane only decides.

1. **The control plane sends the decision, not the arithmetic.** When a grace
   period ends, the rendered tenant `*.toml` gains
   `[limits] registrations = false` and/or `media_uploads = false`, and the
   control plane sends `SIGHUP` (it already reloads tenant configuration this
   way). This works without a restart. Eunha doesn't compare usage with plans
   itself, so there is one source of truth.
2. **Eunha refuses growth with a clear reason:**
   - `registrations = false` closes sign-ups as if registrations were closed,
     with a message for the owner's admins.
   - `media_uploads = false` makes `POST /api/v2/media` (and avatar, header
     and emoji uploads) answer 422 with a message saying the instance is over
     its media allowance.
   - Federation, posting text, reading and moderation are untouched.
3. **Database.** There's nothing to block cleanly. A database grows from
   ordinary posting and federation, and making it read-only is an outage. So
   it stays a warning plus an operator conversation, like activities.
   Revisit if it turns out to be a real cost.
4. Each of these is a behavioural difference from Mastodon, so it gets an
   entry in Eunha's `divergences.toml`, and documentation in
   `docs/operating/`.
5. **Lifting it:** once usage is back under the limit, the next daily check
   (or an owner's "check now" button) renders the flags off and reloads.
   Upgrading to a plan that fits lifts it immediately.

## Billing interactions

- **Upgrade:** lifts any enforcement that the new plan covers, straight away.
- **Downgrade and refund to a smaller plan:** allowed with the warning from
  Phase 2. If the instance is over the new plan, the grace period starts then,
  not at the next daily check. The instance-count refusal stays as it is.
- **Admin and billing-exempt accounts** are measured but never warned or
  enforced, as with the instance count today.

## Open questions for you

1. Grace period length: 14 days, or the rest of the billing period?
2. Do remote-media caches need their own operator-side cap per tenant (for
   example a shorter `media remove` age when one tenant's cache is large)?
3. Should users count active people (signed in within the last 30 days)
   rather than accounts? That is friendlier to communities with many dormant
   sign-ups, but harder to explain.
4. Is database size worth promising at all, or should it come off the plans
   and be covered by fair use, as activities are?
