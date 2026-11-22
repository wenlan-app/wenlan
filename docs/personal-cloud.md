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
  cookie, no other account's pass. The pass also says whether the person's
  browser or one of their computers sent the request; a browser's change
  must come from the personal cloud's own page.
- **Computers.** A computer connects with the team server's short-code flow
  (`POST /auth/device`, then the person types the code at `/app/device`
  while signed in, having signed in again within five minutes) and gets a
  key of its own (`wlc_…`, only its hash kept in `accounts.json`). The
  person sees their computers at `GET /auth/devices` and removes one with
  `DELETE /auth/devices/<id>`; its key is refused from its next request.
- **The library copy.** Each person's Wenlan keeps a copy of their library
  folder (`space.json` and `members/<member>/<kind>/<id>/<sha>.wlv`, memories
  included) under `cloud-library/` in its folder, at
  `/team/v1/library/files` and `/team/v1/library/file?path=…`. A version is
  kept only when its bytes match its name and is never replaced. The cloud
  copy only stores; it never joins the library as a computer.

## Running it

```bash
wenlan-server cloud-add-account --root /srv/wenlan-cloud alice@example.com
wenlan-server gateway https://cloud.example.com --root /srv/wenlan-cloud \
  --listen 0.0.0.0:8443 --max-running 20 --idle-minutes 15
```

Put HTTPS in front of `--listen`. `--root` holds `accounts.json`, the
entrance's key `gateway.key` (readable by its user only) and everyone's
folders; keep it on an encrypted disk.

## The desktop side

Settings › Sync › Wenlan cloud signs in through the browser with the short
code and keeps the computer's key in `cloud-library.json` beside the share
settings (readable by its user only). From then on every library pass makes
the library's engine folder and the cloud hold the same files; `space.json`
follows the side that changed since the last round, and both changed means
their computers together. A computer whose library is synced nowhere else
gets a folder of its own (`cloud-library/` in the data folder), filled from
the cloud first so it joins the library its other computers keep there.
`WENLAN_CLOUD_URL` points it at another cloud. A file removed on one side is
not removed on the other.

Not in the trial yet: AI connectors, documents' original files, backups and
deleting an account.
