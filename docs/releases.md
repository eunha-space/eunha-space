# Releasing eunha to the fleet

A runtime is one eunha process serving many tenants with one binary, so eunha
ships runtime by runtime, never tenant by tenant. A tenant's release is its
runtime's. *src/rollout.rs* rolls each new build out incrementally through
release channels, and the console's **Releases** section is where an operator
watches and steers it.

## Release channels

A release channel is a group of runtimes. Channels have an order, and a build
reaches them in that order. Each channel has a soak: how long it runs a build,
healthy, before the next channel gets it. The last channel never soaks.

Migration 069 creates two channels, `canary` (first, 60-minute soak) and
`stable` (last), and puts every existing runtime in `stable`. With nothing in
`canary`, the fleet behaves as it did before: every runtime takes each build, one
at a time. Put a runtime in `canary`, and move tenants who should see builds
first onto it, to get a canary.

In the console an operator can create a channel (it goes last), rename it,
change its soak, move it earlier or later, and delete it once it has no
runtimes. Each runtime row has a channel selector. A runtime in no channel takes
no automatic release.

A runtime takes part in rollouts when it is in a channel, **Follows rollouts** is
on, and it serves at least one ready tenant, because a release is checked against
one. Turning **Follows rollouts** off pins a runtime to its release: rollouts pass
it by, and **Deploy latest** still deploys it by hand.

## A rollout

The control plane asks CI for the newest build of `main` every five minutes. A
build it has not seen starts a rollout, which supersedes the one in flight. The
new rollout starts again from the first channel, so a fix for a bad build is
canaried like any other. Every runtime a rollout reaches gets that build, the
same commit, however many land while it soaks. Every 30 seconds the rollout moves
along:

1. In the first channel that has not completed, the next runtime by name that
   is not on the build is released. Only one release runs at a time anywhere
   in the fleet, as before.
2. After a release, the control plane asks every tenant on the runtime for its
   health, not only the one the switch was checked against, and records how many
   did not answer healthy.
3. Once every runtime of the channel that takes part runs the build, the channel
   completes, and its soak starts.
4. When the soak ends, the next channel starts. After the last channel the
   rollout is completed.

A rollout holds instead of going on when:

- a tenant on a runtime that already has the build is unhealthy. This holds
  both the rest of the channel and the next channel. A tenant that was unwell
  before the build reached its runtime holds nothing.
- a tenant is moving on or off the next runtime (see
  [tenant moves](tenant-lifecycle.md#moving-a-tenant-to-another-runtime)).
- the runtime's last release of this build failed less than five minutes ago.

A runtime an operator has deployed a newer build to by hand counts as done:
a rollout never takes a runtime back to an older build. Its reason is shown in
the console. A runtime whose build cannot be compared with the rollout's holds
the rollout until it can. A runtime that fails to take the build three
times **halts** the rollout, which then goes no further until an operator acts.

## Which release is newer

A release is a commit on eunha's `main`, and a newer one descends from an older
one. *src/release_order.rs* asks eunha's commit graph rather than comparing
when each was built, because an old commit built for the first time after a
newer one would otherwise look newer. With CI builds it asks GitHub's compare
API with the CI token, which needs read access to eunha's contents; otherwise
it asks the central checkout with `git merge-base --is-ancestor`. Two commits'
order never changes, so each answer is kept in `eunha_release_order` and asked
for once.

## Steering a rollout

- **Pause** stops starting releases; one already running finishes. A pause is a
  freeze: a newer build arriving while paused starts its rollout paused too.
- **Resume** carries on, and gives every runtime its three attempts back, which
  is how a halted rollout is retried.
- **Promote** lets the rollout past the channel it is in. That channel's soak and
  its hold on unhealthy tenants stop applying.

Each is recorded as a `release_policy` operation, as are changes to channels.

## Tables

- `release_channels`: name, `position` (the order) and `soak_minutes`.
  `runtimes.channel_id` points at one.
- `release_rollouts`: one row per build, with `status` (`rolling`, `paused`,
  `halted`, `completed`, `superseded`) and `reason`. Only one is ever `rolling`,
  `paused` or `halted`. Failed releases before `attempts_since` do not count
  against it.
- `release_rollout_stages`: when the rollout reached each channel, completed it,
  and whether it was promoted past it.
- `runtime_deployments.rollout_id`, `tenants_checked` and `tenants_unhealthy`
  link each automatic release to its rollout and record its tenant check.
- `eunha_release_order`: where commit `head` stands against commit `base`
  (`identical`, `ahead`, `behind` or `diverged`), kept from the commit graph.

## What a rollout cannot undo

A release migrates every tenant database on the runtime before it switches, and
a rollback restores the previous binary, not the previous schema. A build
whose migrations break the previous binary cannot be rolled back by a halt.
Give such a build a longer soak by pausing before it lands, or promote with
care.
