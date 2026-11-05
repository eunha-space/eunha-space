# Signing in as a customer

An administrator can see the customer pages as a customer sees them: the
console's Users section (`/admin/users`), which lists every account apart from
billing, has a **Sign in as** button on each verified customer. It is for
support, such as following what a customer reports on their instance page.

## The session

Signing in as a customer makes a separate session of that customer, marked
in `user_sessions.masquerading_admin_id` with the administrator who started
it. It lasts an hour. The administrator's own session is not ended; its
cookie is moved to `eunha_space_admin_session`, which only `/api/customer`
receives. **Back to the control plane** on the customer pages, or signing
out, deletes the masquerade and puts the administrator's session back, if it
is still good; otherwise the administrator signs in again.

A masquerade:

 -  never reaches the console or its API, even if the customer later becomes
    an administrator;
 -  cannot be started for another administrator, or for an account whose email
    is not verified;
 -  ends as soon as the administrator who started it loses the role;
 -  cannot change the customer's password or billing (403), and a language
    picked during it is not saved to the customer's account.

Everything else on the customer pages works, including acting on the
customer's instances. Those operations are recorded as the customer's.

## Audit

Each masquerade records a `masquerade` operation, requested by the
administrator, with the customer's id and email in its detail.
