# Billing

Customers subscribe monthly through TossPayments automatic payments (빌링).
TossPayments stores the card and charges it when asked. It does not schedule
anything itself, so every renewal is started by the control plane.

TossPayments cannot bill cards issued outside Korea. Those go through Polar,
described in [International cards](#international-cards).

## Configuration

Billing is off unless both TossPayments keys, or both Polar settings, are
set. With it off, nobody is billed and creating an instance needs no
subscription.

```sh
TOSSPAYMENTS_CLIENT_KEY=live_ck_...        # public; handed to the billing window
TOSSPAYMENTS_SECRET_KEY=live_sk_...        # process-only
TOSSPAYMENTS_MID=...                       # the secret key's merchant ID; required with a live key
EUNHA_SPACE_SUBSCRIPTION_NAME='은하공간 월간 구독'   # optional order name
TOSSPAYMENTS_API_BASE=https://api.tosspayments.com    # optional; a stand-in for tests
```

The TossPayments contract must have automatic payments enabled for the
client key's merchant ID. A subscription keeps the amount it started at, so a
new price applies only to new and restarted subscriptions.

A billing key can be charged only through the merchant ID (MID) that issued
it. Each subscription records its key's MID, and with `TOSSPAYMENTS_MID` set a
subscription whose key another MID issued is not charged at all: it is logged
and looked at again hourly, and nothing counts against the card. Keys from a
test setup therefore stay uncharged after the switch to live keys, instead of
being declined until their subscriptions end. Subscribers re-register a card to
move to the live MID.

Answers about this control plane's own setup are never counted as declines:
a 401, or codes such as `UNAUTHORIZED_KEY`, `INVALID_API_KEY`,
`NOT_FOUND_MERCHANT`, `NOT_FOUND_TERMINAL_ID` or `INVALID_REQUEST`. The charge
stays pending and is logged as an error, and subscriptions keep running until
the setup is fixed. Without this, one wrong key would decline every renewal and
take every subscriber's instances offline days later.

## Subscribing

1. **Subscribe with a card** on `/account` calls `POST /api/customer/billing/checkout`,
   which returns the client key and the account's `customerKey`, a random UUID
   rather than the user id so it cannot be guessed.
2. The page opens TossPayments' billing window (`requestBillingAuth`, method
   `CARD`). Both its success and failure redirects return to `/account/billing`.
3. On success, the page sends the `authKey` and `customerKey` to
   `POST /api/customer/billing/authorization`. The server checks the customer
   key belongs to the signed-in account, exchanges the `authKey` for a billing
   key, and charges the first month at once.

Registering a card while subscribed replaces the card. While a renewal is being
retried, the new card is charged at once.

The billing key and the secret key never leave the server. No API returns
them, and no log, operation, or operation event contains them. The account page
shows only the card company and the number as TossPayments masks it.

## Renewals

A period runs from `anchor_at + n months` to one month later, so a subscription
started on the 31st renews on the last day of shorter months and goes back to
the 31st after them. The renewer runs with the lifecycle jobs, which
`CONTROL_JOBS_DISABLED` turns off, and checks every 30 seconds for subscriptions
that are due.

An order id is the payment's date in Korea and eight random digits, as in
`2026-11-05-0421-7730`, so it can be read out over the phone. Orders made before
this format keep their `eunha-…` ids.

A charge is written to `subscription_payments` as `pending` before
TossPayments is called. Its order id is also the `Idempotency-Key`, and
`charge_attempted_at` records when it went out. TossPayments keeps a key's first
answer for 15 days, errors included, so a charge whose outcome is unknown (a
timeout, a 5xx response, or a TossPayments-side processing code) is never sent
again under its key. Five minutes later the order is looked up instead. An
approved order is settled as paid, and one TossPayments ended (`ABORTED`,
`EXPIRED`) or that was cancelled in full before the approval was recorded
(`CANCELED`) as declined. One cancelled in part (`PARTIAL_CANCELED`) is settled
as paid, with the part given back recorded as a refund. An order TossPayments has no record of
(`NOT_FOUND_PAYMENT`) 45 minutes after it was sent is `expired`, and a new order
for the same period is charged at once; that is not counted as a decline. A
charge in flight holds a lease, so two control planes cannot send it at once.

Requests to TossPayments time out after 90 seconds, of which connecting may
take 10, and don't reuse connections, as its network requirements ask. An email
address over 100 characters, the most TossPayments takes, is left out of the
billing window and the charge; it only uses it to mail its own receipt.

Renewals and their retries are charged at 09:00 in Korea. One due later than
that waits for the next day's, so a customer whose period ends at 14:00 is
charged at 09:00 the next morning, without the next period moving: its dates
still follow the original one. What a customer is waiting on goes straight away:
a first charge, a new card for a declined renewal, an upgrade, and an operator's
retry. A subscription set to end still ends with its period, and looking up a
charge whose outcome is unknown carries on through the day. Customers see the
charge date on the subscription page and in the renewal reminder.

A declined renewal is retried at 09:00 after 1, 2, and 4 more days. During those retries
the subscription is `past_due`, its instances keep running, and the customer is
emailed about each decline when control-plane SMTP is configured. After the
fourth decline, the subscription is `lapsed`.

A declined first charge is not retried. The customer sees the card company's
reason and can try another card.

## When a subscription ends

A cancellation takes effect when the paid period ends. Until then the customer
can choose **Keep my subscription** to undo it. Cancelling while `past_due`
ends the subscription at once, because its paid period is already over.

When a subscription becomes `canceled` or `lapsed`, each of the customer's
instances is deprovisioned in the usual durable way, with one difference: **no
purge is scheduled**, because nobody confirmed a deletion. The instance becomes
`retained` with `billing_suspended_at` set, keeps all its data, and the sweep
never purges it. The customer can still delete it permanently. An operator can
restore or purge it as with any other tenant.

Subscribing again starts a new period that day. It also queues a restore for
every instance the subscription suspended, up to the customer's instance limit.
An instance that cannot come back stays retained for the customer to restore,
for example one whose custom domain another instance has taken.

A customer needs an `active` or `past_due` subscription to create or restore an
instance. Administrators are never billed. Customers whose instances predate
billing keep them running, but cannot add instances without subscribing.

## Plans

Operators manage plans on `/admin/billing/plans`. A plan has English and Korean names,
a monthly price and an instance limit. With no limit, the account's own
`instance_limit` applies. Administrators and exempt accounts always use their
account's limit.

A plan also states limits on registered users, media and database storage, and
ActivityPub activities a month, shown to customers as a fair-use limit. These are
only displayed so far: nothing measures or enforces them.

Names, the instance limit and the shown limits can be edited at any time; a
shown limit left empty is shown as no limit. A plan nothing has used, no
subscription running or ended and no payment, can be deleted. One that was
used can only be retired, so that its history keeps its name.

A plan's price never changes, because subscriptions and upgrade charges are
worked out from it. To change a price, add a plan and stop offering the old one.
Those on a retired plan stay on it, at their price, until they move.
Without a plan on offer, customers cannot subscribe.

`/pricing` shows the plans on offer to anyone, signed in or not, from
`GET /api/customer/plans`, which needs no session and lists nothing while
billing is off.

Plans are monthly only: a period is always one month.

## Changing plans

An active subscription that is not set to end can move to any offered plan.
Past-due subscriptions must pay first.

- **A dearer plan applies now.** The customer pays the price difference for the
  days left in the period (일할 계산), the day under way counted as left,
  rounded up to ten won, and the renewal date stays the same. An upgrade on the
  day of subscribing costs the whole difference, so the two payments add up to
  the dearer plan's price; 8,000 won more a month with 29 days of a 30-day
  period left is 7,733.33, charged as 7,740. Under
  100 won it applies free. The confirmation names the days and the amount, and
  sends the amount back as `expected_charge`. If a day has turned since and the
  amount is no longer that, nothing is charged and the customer is told the new
  amount, so nobody is charged an amount they were not shown. The charge is a payment with `kind`
  `upgrade`, taking the same order, idempotency and lookup path as a renewal, so
  one interrupted mid-request is never charged twice. If the card declines it,
  the plan does not change, nothing is retried and the decline does not count
  against the subscription.
- **Any other plan applies at the next renewal**, recorded in `pending_plan_id`
  and charged at its price from then on. **Change plan** lists only the other
  plans; **Keep {plan}** beside the scheduled change takes it back. It is refused if the account's instances exceed the new limit, and the
  new limit applies to new instances as soon as the move is scheduled.

## Refunds

A customer can ask for a payment back from the payment history on `/account`
within seven days of paying (`POST /api/customer/billing/payments/{order_id}/refund`).
The full amount is refunded; there are no partial refunds here. The refund
policy at `/refund-policy` (*frontend/src/RefundPolicy.tsx*), linked from the
site footer, the pricing page and the payment history, and the pricing page's
terms state these rates, so change them along with the
window. The policy is written in Korean with an English translation beside it;
change both together. The request is
recorded first and carried out by the renewer, which looks the payment up
before every attempt, so a refund interrupted mid-request is finished later and
never asked for twice:

- A payment TossPayments shows as cancelled is settled as refunded without
  asking again.
- A cancel that gets no clear answer (a timeout, a 5xx, `ALREADY_CANCELED_PAYMENT`,
  a key or merchant setup error) is looked up again five minutes later. Cancels
  carry no idempotency key: TossPayments would replay a refusal under it for 15
  days, and it refuses to cancel a payment twice anyway.
- A refusal on the merits (e.g. `NOT_CANCELABLE_PAYMENT`), a payment taken under
  another MID, or one with no payment key withdraws the request:
  `refund_requested_at` goes back to null and `refund_error` shows the customer
  why, so they can ask again.

While a refund is open the subscription is not charged, and a refund cannot be
asked for while a charge is being confirmed.

What a refund does depends on who gave it back, which is
`refund_requested_by`:

- **A customer's own** full refund of an upgrade takes the upgrade back: the
  subscription returns to the plan and price it moved from (`from_plan_id`,
  `from_amount`) and keeps running. Only the latest upgrade of the period being
  served can be asked back this way, while the subscription is still on its
  plan; an earlier one becomes askable once the later ones are refunded. The
  plan it returns has to hold the account's instances, or the request is refused
  until some are deleted. The billing view carries what a refund would do, so
  the confirmation says it before asking: `refund_plan_en`/`refund_plan_ko`,
  the plan an upgrade's refund returns, and `refund_crowded`, set when that
  plan is too small, which disables the confirmation and says why. Upgrades from a period already over are an operator's
  call.
- **A customer's own** full refund of any other payment ends a running
  subscription at once (`canceled`), retires its card and takes its instances
  offline with their data kept. Subscribing again starts a new period that day
  at today's price and serves them again.
- Customers refund **latest payment first**: a payment can be asked back only
  when every later one is refunded or failed (`LATEST_FIRST`). So no refund
  takes another payment with it, and the confirmation's amount is all that
  goes back. A renewal with an upgrade after it waits until the upgrade is
  refunded, which returns the plan before it.
- **An operator's**, from `/admin/billing` or the TossPayments dashboard, whose
  webhook below brings it into the ledger, returns the money and changes nothing
  else. The plan, the renewal and the instances all stay as they were, so the
  subscription charges again at its next renewal.

## Webhooks

Point TossPayments' webhook at `/api/webhooks/tosspayments` for
`PAYMENT_STATUS_CHANGED` and `BILLING_DELETED`.

Every webhook is kept in `toss_webhook_deliveries` with its headers and what it
was answered, for five years. Billing keys, the `authorization` header and
cookies are removed first. The Reconciliation page shows the latest and whether they
were signed. TossPayments' documentation disagrees about whether these events
are signed, and it is not yet known whether they are under live keys.

Payment events are treated as unsigned, so their content is only a reason to look the
payment up. A cancellation found that way is recorded as above. A change to a
charge still being confirmed, a cancellation included, brings the renewer's
next look at it forward. An
event the control plane cannot look up is answered with a 503, so TossPayments
sends it again.

`BILLING_DELETED` cannot be looked up, but it names the billing key, which only
TossPayments and this control plane know. All it does is stop charging that
key. A running subscription keeps its paid period and ends with it unless the
customer registers a new card, which keeps it going. The customer is emailed.

## Deleting billing keys

A billing key stays chargeable for as long as its card is valid. When a
subscription ends, or a new card replaces the old one, the old key goes to
`retired_billing_keys`, and the renewer deletes it at TossPayments
(`DELETE /v1/billing/{billingKey}`), retrying after an hour, doubling up to a
day. Once TossPayments confirms, or answers `NOT_FOUND_BILLING`, the key itself
is erased. A key from another MID cannot be deleted with this one's secret key;
it waits with `last_error` set, for deletion in the TossPayments dashboard.

## Operators

The billing section of the console has four pages, listed under **Billing**
in the sidebar:

- **Overview**, `/admin/billing`: the **New subscriptions** switches, then
  monthly revenue and counts of active,
  trialing and canceling subscriptions and of accounts running instances
  unbilled (such as customers from before billing). Under **Needs
  attention** are past-due subscriptions, open refunds, TossPayments
  mismatches and retired card keys TossPayments has not confirmed deleting
  after three tries. Selecting a count opens the page that lists them.
- **Subscribers**, `/admin/billing/subscribers`: every account that
  subscribed, is exempt, or runs instances unbilled, with its standing, card,
  renewal date and instances. Selecting one shows its payments.
- **Plans**, `/admin/billing/plans`: the plans and their Polar products.
- **Reconciliation**, `/admin/billing/reconciliation`: the daily comparison
  with TossPayments, its mismatches and the latest webhooks.

From the Subscribers page an operator can:

- **Charge again now** a past-due subscription, or a first charge that was
  declined, on the card on file. A decline counts as one of the retries.
- **Refund** any paid payment, outside the customer's seven days too, through
  the same pipeline as a customer's refund.
- **Exempt** an account from billing (`users.billing_exempt`). Like an
  administrator, it can create and restore instances without a subscription,
  and a subscription of its that ends takes nothing offline. Lifting the
  exemption takes nothing offline either: the account just cannot add or
  restore instances until it subscribes.

Each of these is recorded as an operation attributed to the operator. The page
never shows a billing key or customer key.

### Pausing new subscriptions

The **New subscriptions** switches on the Overview pause sign-ups through
TossPayments (Korean cards) and Polar (international cards) separately,
kept in `billing_settings` and set with `PATCH /billing/signups`. A paused
provider refuses a checkout, and a Korean card registered for a subscription
that is not running, with a `409`. The account page drops that provider's
button, and says new subscriptions are paused when neither is left. That
includes someone subscribing again after their subscription ended.

Nothing that runs is touched: renewals, retries, plan changes, card changes,
cancelling and resuming go on as before, as do webhooks for a Polar checkout
started before the pause. Administrators can still subscribe, to test with
real cards before reopening. Each change is recorded as a
`subscription_signups` operation.

## Emails

Billing emails go out through an outbox, `billing_emails`. Each is written in
the same transaction as what it is about, with a dedupe key, so a receipt exists
exactly when its charge was recorded as paid and is never written twice. The
renewer sends what is due through control-plane SMTP, backing off from five
minutes to six hours, and gives up on an email it could not send within three
days rather than deliver one about something long past. Without SMTP
configured, nothing is sent.

- **Receipt**, for every paid charge: the amount, the card's company and last
  digits, the period, the order id and TossPayments' receipt link.
- **Renewal reminder**, seven days before a subscription renews: the date, the
  amount and the card, and how to cancel or change the card. Not sent for a
  subscription set to end, or one renewing within a day.
- **Refund confirmation**, for every refund recorded. After a full refund it says
  that the subscription will not be charged again and the day its instances go
  offline.

Every billing email is written in the account's language and sent under the
service's name in it: 은하공간 for Korean, eunha space for English, from the
address in `EUNHA_SPACE_EMAIL_FROM`.

Declines, a subscription ending, and a card removed at TossPayments are still
emailed directly, best effort; the account page says the same.

## Audit

Each card registration, cancellation, resumption, charge, refund request,
refund, and subscription end is recorded as an operation with no tenant (`subscription_*`), attributed to the
customer. Charges record the order id, amount, period, and the decline code.
`subscription_payments` keeps every charge, including the TossPayments payment
key and receipt URL. Customers see their receipts on the account page.

## Comparing with TossPayments

The other paths only ask about orders still open. So once a day, after 04:00 in
Korea, the renewer compares the day before with `GET /v1/transactions`. A control
plane that was down catches up on up to seven days, and a failed comparison is
tried again after an hour. `toss_transaction_checks` records each day.

A cancellation the books have not heard of, because its webhook never came, is
recorded like any other. Anything else is written once to `toss_mismatches` and
logged as an error, and nothing is changed: only a person can tell whether a
charge should be honoured or given back. They appear on the Reconciliation page until
marked resolved.

| Kind                    | Meaning                                                                     |
| :---------------------- | :-------------------------------------------------------------------------- |
| `approved_not_recorded` | Closed here as `failed` or `expired`, but TossPayments holds the money.     |
| `unknown_order`         | TossPayments moved money on an order the books don't have.                  |
| `amount_differs`        | The approved amount isn't the recorded one.                                 |
| `not_at_toss`           | Recorded as paid, but TossPayments has no such payment.                     |
| `still_pending`         | No outcome a day after the charge. Resolves itself once settled.            |

Payments under another MID are skipped, because the current key can't see them.
Test orders from other machines sharing a test key appear as `unknown_order`.

## International cards

Cards issued outside Korea pay through [Polar](https://polar.sh), the
merchant of record. Polar sells the subscription in its own name, collects
and remits the tax, keeps the renewal schedule and retries failed charges. It
also mails the receipts and failed-payment notices, so no receipt is written
here for a Polar payment.

```sh
POLAR_ACCESS_TOKEN=polar_oat_...           # an organization token; process-only
POLAR_WEBHOOK_SECRET=whsec_...             # the webhook endpoint's secret
POLAR_CURRENCY=usd                         # optional; the one currency plans are sold in
POLAR_API_BASE=https://api.polar.sh        # optional; https://sandbox-api.polar.sh, or a stand-in
```

The token needs `checkouts:write`, `subscriptions:write`, `orders:read`,
`refunds:write`, `products:read` and `customer_sessions:write`. Every request
pins `Polar-Version: 2026-10`. Polar removes a version about nine months
after it is released, and then answers it with 404. A 404 that does not name a
version is treated as Polar being unreachable, not as a missing object.

Set Polar up like this:

- Add a webhook endpoint at `https://eunha.space/api/webhooks/polar` in Raw
  format, pinned to API version 2026-10, and subscribe it to the
  `subscription.*`, `order.*`, `refund.*` and `product.*` events.
- In the customer portal settings, turn off **Enable subscription plan
  changes** and **Enable subscription pause**. Plan changes are made on the
  account page, which checks the instance limit before a downgrade. A paused
  subscription is treated as ended, and a resumed one as a stray to revoke.

### Polar plans

A plan is sold to international cards once an operator links it to a Polar
product on the billing page. The product must renew every month at a fixed
price in `POLAR_CURRENCY`. Its price is read from Polar and kept in
`plans.polar_amount`, in the currency's smallest unit (cents).

Unlike the won price, the Polar price can change. Polar lets a fixed price
be changed, and existing subscribers keep the price they signed up at. Their
amount comes from their own Polar subscription, so they are not affected
here either. A `product.updated` webhook reads the product again, and
**Refresh** on the billing page does the same without waiting for it. What
new customers see, and whether a plan change applies now or at the renewal,
follows the new price. A subscriber sees their own price on their current
plan. A product that can no longer be sold as a plan, because it was
archived or lost its monthly USD price, keeps its last price and is logged.

**Change** moves a plan to another Polar product, as when switching from
the sandbox to live products, and reads the new product's price. A plan
with a running Polar subscription keeps its product, because those
subscriptions were sold as it. To switch from the sandbox, end the test
subscriptions first. A sandbox subscription left running would be looked up
against production every hour and logged as unknown.

### Free trials

A trial is set on the Polar product, never here: Polar takes the card at
checkout, charges nothing until the trial ends, emails a reminder three days
before, and then bills the first period. The checkout does not override the
product's trial. Linking a product, its `product.updated` webhook and
**Refresh** read its trial into `plans.polar_trial_interval` and
`polar_trial_interval_count`, which Pricing, the account page and the
billing page show. TossPayments has no trial, so Korean cards pay from the
first day.

A subscription Polar calls `trialing` runs its instances like an active one.
Its `trial_started_at` and `trial_ends_at` are mirrored while the trial
lasts. The account page then names the trial's end as the first charge. The
billing page counts it under **In a free trial** rather than **Active**, and
leaves it out of the monthly revenue. Cancelling during a trial ends the
subscription when the trial ends, with nothing charged, and its instances go
offline as after any cancellation. Turn on **Prevent trial abuse** in Polar's
subscription settings so that a customer gets one trial only.

### Subscribing through Polar

The account page offers **Pay with an international card** for a plan that
has a Polar product. The control plane creates a checkout session with the
product, the account's id as `external_customer_id`, its email, the currency,
and the visitor's address from `cf-connecting-ip`, so that Polar can work out
the tax. The customer pays on Polar's page. Polar then sends them back to
`/account/billing?polar_checkout=…`, and the page calls
`POST /customer/billing/polar/sync` so that it does not have to wait for the
webhooks.

A customer has one subscription row, whichever provider bills it. A Polar
checkout is refused while a TossPayments subscription runs, and a
TossPayments card cannot be added while a Polar one runs. A Polar
subscription that starts over an ended TossPayments row takes the row over,
and its old billing key is retired.

### Keeping in step with Polar

A Polar webhook is a notification, not a fact. Its Standard Webhooks
signature is checked against both key schemes. Secrets made before 8
September 2026 sign with the secret's own bytes; newer ones sign with the key
it encodes. Timestamps more than five minutes off are refused. The
subscription or order the webhook names is then read from Polar, so events
that arrive late or out of order settle on Polar's current record. Every
delivery is kept in `polar_webhook_deliveries`. One whose signature fails is
answered 401 and not acted on.

Running subscriptions are also read from Polar hourly, in case a webhook
never arrived.

| Polar status             | Here                                                         |
| :----------------------- | :----------------------------------------------------------- |
| `active`, `trialing`     | `active`                                                     |
| `past_due`               | `past_due`; instances keep running while Polar retries       |
| `canceled` with an end   | `lapsed` if it was past due here, otherwise `canceled`       |
| `unpaid`                 | `lapsed`                                                     |
| `incomplete`             | nothing yet                                                  |

Polar retries a declined renewal after 2, 7, 14 and 21 days, then revokes the
subscription. That is longer than the TossPayments schedule, and the
instances keep running throughout. Cancelling sets `cancel_at_period_end` at
Polar, and Polar ends the subscription when the period ends. Cancelling a past
due subscription revokes it at once. A second subscription that runs at Polar
for an account that already has one is left alone. An operation records it,
so that an operator can refund and revoke it in Polar's dashboard.

### Plan changes, refunds and the portal

An upgrade is sent with `proration_behavior: invoice`, so Polar charges the
difference at once and applies the change only if that charge succeeds. A
downgrade is sent with `next_period`, and choosing the current plan again
clears the pending update. The plan has to be sold in the subscription's
currency.

During a free trial every change applies at once, a cheaper plan included,
and is sent with `invoice`. Polar carries the trial over to the new product
and recomputes its end from the trial's start with that product's trial
length, so nothing is charged. If the new product's trial is shorter than
the time already used, or it has none, the trial ends and Polar charges the
new plan's price at once. The confirmation says which, from the plan's
`trial_until`.

A customer can ask back only the account's first Polar payment (its first
`renewal`, which after a free trial is the first charge), within seven days,
and only if the account has never had a refund of its own. Polar keeps its
fee on every refund, and the trial is where an international card tries the
service, so this is for one who forgot to cancel. Any other Polar payment is
an operator's call from `/admin/billing`, which is bound by none of this.
TossPayments payments are not limited this way. The rule is
`POLAR_SELF_REFUND` in *src/billing_sync.rs*, shown on Pricing and the
refund policy. As with TossPayments, a full refund ends the subscription and
takes the instances offline. `refund_sent_at` is written before Polar is asked, so the refund is
never requested twice. After that, Polar's own record of the order and its
refunds decides the outcome. The request is sent again only if Polar has no
refund for the order an hour later. The net amount is refunded and Polar adds
the tax. Polar's fee is not returned. Once a refund has ended the
subscription here, the subscription is revoked at Polar, and that is retried
until Polar agrees. Polar may also refund on its own to head off a chargeback.
That refund is recorded, and the subscription ends when Polar revokes it.

**Manage billing** opens Polar's customer portal through a customer session.
There the customer changes their card and finds their invoices.
