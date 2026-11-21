# Personal cloud (trial)

The personal cloud runs each person's Wenlan on a server, so their pages are
there when their computers are off. It is in a closed trial; nothing here is
offered publicly yet.

## How it fits together

```
browser / desktop / AI tools
          │
   entrance (wenlan-server gateway)
   sign-in, account table, pass
          │  "where is this person's Wenlan?"
   locator: one process per person, stopped when idle
          │
   Alice's Wenlan   Bob's Wenlan …   (each its own folder)
```

- **Entrance.** People sign in with Google or GitHub, using the same
  settings as a team server (`WENLAN_TEAM_GOOGLE_CLIENT_ID` and
  `WENLAN_TEAM_GOOGLE_CLIENT_SECRET`, `WENLAN_TEAM_GITHUB_CLIENT_ID` and
  `WENLAN_TEAM_GITHUB_CLIENT_SECRET`, or `team-auth.json` in `--root`). The
  provider's callback address is `<url>/auth/callback/<provider>`.
- **Accounts** are made ahead of time, one per email. The first sign-in
  whose provider vouches for that email (a verified GitHub address, or a
  Gmail or company Google account) is attached to it; after that only that
  provider account opens it. Account ids are random and are the only name a
  person's folder goes by.
- **Each person's Wenlan** is `wenlan-server team` with its own folder
  (`<root>/people/<account id>`) and ports on loopback. It is started on
  their first request, stopped after `--idle-minutes` without one, and runs
  no background work (no sorting memory, no models, no scheduler).
- **The pass.** The entrance forwards each request with a pass that names
  the account and expires within a minute, signed with a key derived for that
  account alone. A person's Wenlan takes nothing else: no member key, no
  cookie, no other account's pass.

## Running it

```bash
wenlan-server cloud-add-account --root /srv/wenlan-cloud alice@example.com
wenlan-server gateway https://cloud.example.com --root /srv/wenlan-cloud \
  --listen 0.0.0.0:8443 --max-running 20 --idle-minutes 15
```

Put HTTPS in front of `--listen`. `--root` holds `accounts.json`, the
entrance's key `gateway.key` (readable by its user only) and everyone's
folders; keep it on an encrypted disk.

Not in the trial yet: connecting the desktop app, AI connectors, backups and
deleting an account.
