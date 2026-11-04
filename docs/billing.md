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
EUNHA_SPACE_SUBSCRIPTION_AMOUNT=9900       # won per month, required with the keys
EUNHA_SPACE_SUBSCRIPTION_NAME='eunha space 월간 구독'   # optional order name
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

## Audit

Each card registration, cancellation, resumption, charge, and subscription end
is recorded as an operation with no tenant (`subscription_*`), attributed to the
customer. Charges record the order id, amount, period, and the decline code.
`subscription_payments` keeps every charge, including the TossPayments payment
key and receipt URL. Customers see their receipts on the account page.
