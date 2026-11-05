# Billing

Customers subscribe monthly through TossPayments automatic payments (빌링).
TossPayments stores the card and charges it when asked. It does not schedule
anything itself, so every renewal is started by the control plane.

## Configuration

Billing is off unless both keys are set. With it off, nobody is billed and
creating an instance needs no subscription.

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

A charge is written to `subscription_payments` as `pending` before
TossPayments is called. Its order id is also the `Idempotency-Key`, and
`charge_attempted_at` records when it went out. TossPayments keeps a key's first
answer for 15 days, errors included, so a charge whose outcome is unknown (a
timeout, a 5xx response, or a TossPayments-side processing code) is never sent
again under its key. Five minutes later the order is looked up instead. An
approved order is settled as paid, and one TossPayments ended (`ABORTED`,
`EXPIRED`) as declined. An order TossPayments has no record of
(`NOT_FOUND_PAYMENT`) 45 minutes after it was sent is `expired`, and a new order
for the same period is charged at once; that is not counted as a decline. A
charge in flight holds a lease, so two control planes cannot send it at once.

A declined renewal is retried after 1, 2, and 4 more days. During those retries
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

Operators manage plans on `/admin/billing`. A plan has English and Korean names,
a monthly price and an instance limit. With no limit, the account's own
`instance_limit` applies. Administrators and exempt accounts always use their
account's limit.

A plan also states limits on registered users, media and database storage, and
ActivityPub activities a month, shown to customers as a fair-use limit. These are
only displayed so far: nothing measures or enforces them.

A plan's price never changes, because subscriptions and upgrade charges are
worked out from it. To change a price, add a plan and stop offering the old one.
Those on a retired plan stay on it, at their price, until they move.
Without a plan on offer, customers cannot subscribe.

`/pricing` shows the plans on offer to anyone, from
`GET /api/customer/plans`, which needs no session and lists nothing while
billing is off.

Plans are monthly only: a period is always one month.

## Changing plans

An active subscription that is not set to end can move to any offered plan.
Past-due subscriptions must pay first.

- **A dearer plan applies now.** The customer pays the price difference for the
  time left in the period, rounded up to the won, and the renewal date stays the
  same. Under 100 won it applies free. The charge is a payment with `kind`
  `upgrade`, taking the same order, idempotency and lookup path as a renewal, so
  one interrupted mid-request is never charged twice. If the card declines it,
  the plan does not change, nothing is retried and the decline does not count
  against the subscription.
- **Any other plan applies at the next renewal**, recorded in `pending_plan_id`
  and charged at its price from then on. Choosing the current plan takes it
  back. It is refused if the account's instances exceed the new limit, and the
  new limit applies to new instances as soon as the move is scheduled.

## Refunds

A customer can ask for a payment back from the payment history on `/account`
within seven days of paying (`POST /api/customer/billing/payments/{order_id}/refund`).
The full amount is refunded; there are no partial refunds here. The refund
policy on the subscription page (*frontend/src/RefundPolicy.tsx*) and the
pricing page's terms state these rates, so change them along with the
window. The request is
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

- **A customer's own** full refund ends a running subscription at once
  (`canceled`), retires its card and takes its instances offline with their data
  kept, whichever payment it was. Subscribing again starts a new period that day
  at today's price and serves them again. Refunding the renewal for the period
  being served also requests a refund of any upgrade charged for that period.
- **An operator's**, from `/admin/billing` or the TossPayments dashboard, whose
  webhook below brings it into the ledger, returns the money and changes nothing
  else. The plan, the renewal and the instances all stay as they were, so the
  subscription charges again at its next renewal.

## Webhooks

Point TossPayments' webhook at `/api/webhooks/tosspayments` for
`PAYMENT_STATUS_CHANGED` and `BILLING_DELETED`.

Payment events are unsigned, so their content is only a reason to look the
payment up. A cancellation found that way is recorded as above. A change to a
charge still being confirmed brings the renewer's next look at it forward. An
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

`/admin/billing` shows monthly revenue and counts of active, past-due and
canceling subscriptions, accounts running instances unbilled (such as customers
from before billing), open refunds, and retired card keys TossPayments has not
confirmed deleting after three tries. Below that is every account that
subscribed, is exempt, or runs instances unbilled, with its standing, card,
renewal date and instances. Selecting one shows its payments.

From there an operator can:

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
