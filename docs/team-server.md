# Team server

A team server is one Wenlan that stays on (a small Linux box, a Mac mini, a
Docker container) and hosts some of its Spaces for a team. Everyone connects
to it with their own key:

```text
Amy's Wenlan app ──┐                   ┌── Wenlan on the server
Bo's Wenlan app  ──┼── team listener ──┤   (its own routes stay on the server)
Cy's AI tool     ──┘   /team/v1, /mcp  └── team-spaces/<Space>/
```

- **Desktop app members** keep their own copy of the Space. Every ~30 seconds
  their Wenlan takes the team's new page versions from the server and sends
  their own. It works offline and catches up later.
- **AI-tool-only members** (no Wenlan installed) open their invite link in a
  browser. It shows the server address and setup for Claude Code, Codex and
  Cursor. What their AI tool changes is recorded as theirs ("Cy's AI tool").
- **One person, many computers**: every computer and AI tool of one person
  uses the same key and is the same member, so their sentences stay theirs
  on every computer.

For two people, or one person's own computers, a shared folder in Google
Drive or iCloud is enough (Space menu, *Share with people…*). A server adds
real per-person keys, removal and read-only members.

## What is and is not reachable

The team listener is a second listener, separate from the daemon's own port.
It serves only:

| Path | What |
|---|---|
| `GET /health` | `{"status":"ok"}`, no key needed |
| `GET /join` | where an invite link opens in a browser: the team's name and what to do with the link, no key needed |
| `GET /team/v1/me` | who the key belongs to and their Spaces |
| `/team/v1/admin…` | the owner only (403 for anyone else): the team's people, name, invites and new Spaces, for an owner who manages a server nobody sits at from their own computer |
| `GET /team/v1/spaces/{space}/files`, `…/file?path=` | the Space's page versions |
| `PUT /team/v1/spaces/{space}/versions/{page}/{sha}.wlv` | the member's own new version |
| `/mcp`, `/mcp/{space}` | MCP for AI tools, pinned to that Space |
| `/app`, `/auth/…`, `/team/v1/account/…` | with sign-in on (see *Sign in with Google or GitHub*): the team's pages in a browser, signing in, a member's own AI tools |

Every request except `/health`, `/join` and signing in needs a member key
(`Authorization: Bearer <key>`) or, in a browser, a signed-in session.
A member can only see the Spaces given to them, can only send versions
written as themselves, and cannot send anything to a Space they may only
read. The daemon's own routes (settings, other Spaces, memories, shutdown)
are never served on the team listener.

**Removing a member** stops their key at once. Copies of pages already on
their computer stay there; no sync system can take those back.

## Encryption

The team listener speaks plain HTTP. Do one of these:

- **Tailscale (simplest)**: put the server and every member on the same
  tailnet and use `http://<server>.<tailnet>.ts.net:7879` or its `100.x`
  address. Traffic is encrypted by Tailscale.
- **HTTPS**: put a reverse proxy (Caddy, nginx, Cloudflare Tunnel) in front
  and use `https://wenlan.example.com`. The Docker setup below has one built
  in (see *More options* below): Caddy gets the certificate and renews it,
  and invite links use `https://wenlan.example.com`.

A member's Wenlan refuses a plain `http://` address unless it is inside a
private network (this computer, a LAN address, `.local`, Tailscale). Put
the team listener on the open internet only behind one of these: on its
own it does not time out slow or idle connections.

## Set up the server

There are two ways. Both end the same way: people open an invite link.

### A computer that stays on, with the Wenlan app (Mac mini, office PC)

In Settings › Team, choose **Start a team on this computer…**, then type the
team's name, your own name (everyone sees it beside what you write) and the
address others reach this computer at. Wenlan suggests one: the computer's
Tailscale address first, else its address on the local network. Choose
**Start**.

The team listener is on from then on, after restarts too, on port 7879. Turn
it off (and on again) in the same place; the team, its people and Spaces
stay. Invite people from the switch at the top left (**Invite someone…**),
and make team Spaces with **+ New** on the team's side; open each one to
everyone, or to some people, from the line under its title.

On Windows, Windows asks once when the team starts (its own security
alert, naming `wenlan-server`). Choose **Allow**. If the alert is missed or
cancelled, Windows Firewall blocks teammates, and Settings › Team says
other computers can't connect yet, with an **Allow** button. It asks
Windows' administrator prompt to remove the block rules Windows made for
`wenlan-server.exe` and add one inbound rule, **Wenlan team**, for that
program on TCP: private and domain networks, and public ones when the
computer is on one.

### A box nobody sits at (Docker on a NAS or a small cloud machine)

1. On the box, start it with the address people reach it at:

   ```bash
   docker run -d --name wenlan --restart unless-stopped -p 7879:7879 \
     -v wenlan-data:/data ghcr.io/wenlan-app/wenlan-server team https://wenlan.example.com
   docker logs wenlan
   ```

   The address goes in every invite link: the box's `https://` address
   behind a reverse proxy, or its Tailscale address such as
   `http://nas.tailnet.ts.net:7879`. Inside a container Wenlan cannot see
   the box's own address, so give it. Only the team port (7879) is
   published; the daemon's own port listens only inside the container.

   Easier: Settings › Team › *Run it on a server* makes the line for you,
   ending in `--owner <SHA-256>`: your Wenlan makes the owner's key, keeps
   it, and puts only its SHA-256 in the line, so the key is never on the
   server's command line. Once the server is up, that window joins it by
   itself, and step 2 is done.

2. Started with just the address, the first start prints **the owner's
   invite link** once, in the logs. Open it in your own Wenlan (Settings ›
   Team › Join with an invite link). You are the team's owner: from your
   own computer you name the team, invite people,
   and make team Spaces (**+ New** on the team's side). The server makes the
   Space and your Wenlan syncs it like everyone else's. A new Space is only
   yours until you open it to everyone, or to some people, from the line
   under its title. The team sees you by the name in your Wenlan profile.

   Lost the link? Start the container once with
   `-e WENLAN_TEAM_NEW_OWNER_LINK=1` and just the address: a new one is
   printed and the old one stops working.

**More options** with [`docker/team/compose.yaml`](../docker/team/compose.yaml):

- *Automatic HTTPS.* Point a domain at the box, open ports 80 and 443, then
  `WENLAN_DOMAIN=wenlan.example.com docker compose --profile https up -d`.
  Caddy gets the certificate and renews it.
- *Data in a folder on the box* (a NAS share) rather than Docker's own
  volume: swap the volume line in the compose file for the folder and
  uncomment `user:`, set to the folder owner's ids from `ls -n` (with
  `docker run`: `--user <uid>:<gid>`). The image's own user cannot write a
  folder that belongs to someone else on the host; without it the server
  stops at start and says so.

- *No internet on the box.* Bring the image over as a file: on a machine
  with internet, `docker pull ghcr.io/wenlan-app/wenlan-server` and
  `docker save ghcr.io/wenlan-app/wenlan-server -o wenlan-server.tar`; on
  the box, `docker load -i wenlan-server.tar`, then the same `docker run`.
  The server works without the internet from then on. Its search model
  (about 210 MB) is downloaded on first use; until it is there, search is
  by keywords only. To bring it over too, copy the `fastembed_cache` folder
  of any Wenlan that has it (`~/.local/share/wenlan/memorydb/` on Linux,
  `%LOCALAPPDATA%\wenlan\memorydb\` on Windows) into the container and
  restart it:
  `docker cp fastembed_cache wenlan:/data/wenlan/memorydb/ && docker restart wenlan`.

### Keep it running

A restart of the box brings the server back by itself. Two things still
need a person:

- **Updates.** The server does not update itself. When a new Wenlan is out,
  pull the image and start the container again with the same line:

  ```bash
  docker pull ghcr.io/wenlan-app/wenlan-server
  docker rm -f wenlan
  # then the same docker run line as above
  ```

  With the compose file: `docker compose pull && docker compose up -d`.
  The team, its people and pages live in the `wenlan-data` volume and stay.
- **Backups.** Everything is in the `wenlan-data` volume on that box. Back it
  up with the host's own snapshots, or copy the volume. Each member's
  computer also keeps a full copy of the Spaces open to them.

### Going back to an older Wenlan

The first time the team holds something older versions can't (a member who
signed in with Google or GitHub, an email invite, an AI tool key), the
server keeps a copy of `team.json` as `team.json.v1.bak` and writes the new
format. Wenlan 0.22.2 and later read both. Wenlan 0.22.1 and older refuse
the new one, and the team stops answering.

To go back anyway: stop the server, put `team.json.v1.bak` in place of
`team.json`, and start the older version. Everything since that copy is
lost: members added after it, linked Google and GitHub accounts, email
invites and AI tool keys. Pages are not affected.

### If the owner loses access

- **The owner's computer or invite link is lost** (a server nobody sits
  at): start the container once with `-e WENLAN_TEAM_NEW_OWNER_LINK=1`. A
  new owner link is printed and the old owner key stops working.
- **The owner's Google or GitHub account was taken over**: first secure it
  at Google or GitHub. Then restart the server with no sign-in set up
  (move `team-auth.json` aside, or drop the variables): a restart ends every
  browser session, and with sign-in off nobody can start a new one. Start it
  once more with `WENLAN_TEAM_NEW_OWNER_LINK=1` and join with the new link.
  Remove the owner's AI tools with `wenlan team keys` and
  `wenlan team revoke-key`. Turn sign-in back on when the account is safe.
  This version has no way to unlink the owner's own Google or GitHub
  account.

### From a terminal

Everything above can be done with commands on the computer that runs the
team:

```bash
wenlan team start "Acme" --as "Qi"
wenlan team share "Product" --everyone
wenlan team invite "Amy"
wenlan team invite "Bo" --access Product:read
```

`start` turns the team listener on, now and after restarts, and picks the
address for invite links (the computer's Tailscale address first, else its
address on the local network); give another with `--url https://wenlan.example.com`.
`host` lets the team use a Space: `--everyone` opens it to every member, now
and later (`--everyone read` for read only); without it only the people given
it with `--access` can use it. Each `invite` prints an invite link once, such
as `https://wenlan.example.com/join#wlt_…`. Send it to that person privately.
The key stays after `#`, so a browser opening the link never sends it to the
server. Older links without `/join` still work.

`wenlan team status` shows the team, its Spaces and its people.
`wenlan team update Amy --access Product` changes what someone can use,
`wenlan team remove Amy` removes them, and `wenlan team new-key Amy` replaces
a lost link. People are named by their name; when two people have the same
name the command says so and lists their ids. `wenlan team stop` turns the
listener off and keeps the team, its people and Spaces. Renaming a hosted
Space renames it for the whole team: access goes with it, and members'
copies follow by themselves. A desktop install can also set
`WENLAN_TEAM_ADDR` for the background service instead of `wenlan team start`.

## Sign in with Google or GitHub

Without this, people join with an invite link, and the link is their key.
With it, they sign in with Google or GitHub instead: in a browser at
`https://wenlan.example.com/app`, with no Wenlan installed; an owner can
invite someone by email; and `wenlan login` connects an AI tool with a short
code (see *Join*). People who joined with a link keep working, and can add
Google or GitHub from their account menu.

It needs two things, and is off until both are there:

- **An `https://` address** for the team (`WENLAN_TEAM_URL`, or the address
  it was started with). A Tailscale `http://` address is not enough: the
  sign-in cookie only works over HTTPS. Behind Caddy (the `https` profile
  above) this is already true. Google refuses a bare IP address, even over
  HTTPS, so use a name.
- **Google, GitHub or both** set up as below. Either one alone is enough.

### Google

1. In the [Google Cloud console](https://console.cloud.google.com/), make
   a project (or pick one), then open *APIs & Services › OAuth consent
   screen*. Choose **External**, give the app a name people will recognize
   (the team's), and add yourself as a test user while you try it.
   Publish it when other people should sign in.
2. Open *Credentials › Create credentials › OAuth client ID*. Choose **Web
   application** and add this **authorized redirect URI**, with your own
   address:

   ```text
   https://wenlan.example.com/auth/callback/google
   ```

3. Copy the client ID and the client secret.

Wenlan asks Google only for `openid` and `email`.

### GitHub

1. On GitHub, open *Settings › Developer settings › OAuth Apps › New OAuth
   App* (or the same under an organization).
2. Homepage URL: `https://wenlan.example.com`. Authorization callback URL:

   ```text
   https://wenlan.example.com/auth/callback/github
   ```

3. Register it, then make a client secret. Copy the client ID and the
   secret.

Wenlan asks GitHub only to read the profile and the email addresses
(`read:user user:email`). A GitHub account is kept by its number, so a
renamed account still signs in.

### Give them to the server

Either way works; the environment wins when both are set.

- **A file** next to `team.json` in the data folder, named
  `team-auth.json` (in Docker: `/data/wenlan/team-auth.json` in the
  volume). Make it readable by the server's user only (`chmod 600`); the
  server warns when others can read it.

  ```json
  {
    "google": { "client_id": "…apps.googleusercontent.com", "client_secret": "…" },
    "github": { "client_id": "Ov23…", "client_secret": "…" }
  }
  ```

- **Environment variables**: `WENLAN_TEAM_GOOGLE_CLIENT_ID`,
  `WENLAN_TEAM_GOOGLE_CLIENT_SECRET`, `WENLAN_TEAM_GITHUB_CLIENT_ID`,
  `WENLAN_TEAM_GITHUB_CLIENT_SECRET`. With Docker, keep them in a file
  only you can read and pass `--env-file wenlan.env` (with the compose
  file: `env_file: wenlan.env`), so the secrets are not on the command
  line.

Restart the server. Its log says `[team] sign-in is on`, and
`https://wenlan.example.com/auth/providers` lists the ones it offers. If
only one half of a pair is given, the log names the missing one. The
invite page and `/app` then show the sign-in buttons, and the owner's
**Invite someone…** asks for an email.

### Try it on one computer first

Google and GitHub both accept `https://localhost`, so a test needs no
domain. With [mkcert](https://github.com/FiloSottile/mkcert) and Caddy:

```bash
mkcert -install && mkcert localhost      # makes localhost.pem and localhost-key.pem
cat > Caddyfile <<'CADDY'
localhost:8443 {
	tls localhost.pem localhost-key.pem
	reverse_proxy 127.0.0.1:7879
}
CADDY
caddy run &
```

Start the team with `https://localhost:8443` as its address, and use
`https://localhost:8443/auth/callback/google` (and `…/github`) as the
redirect URIs of a separate test OAuth app. Then open
`https://localhost:8443/app`.

### What signing in does and doesn't do

- A browser session lasts until sign-out, **Sign out everywhere**, or a
  restart of the server: sessions are kept in memory, so a restart signs
  everyone out of the browser (AI tools and desktop apps keep working).
- Making something that lasts (an AI tool key, a desktop link) asks the
  person to sign in again first if their last sign-in is more than five
  minutes old. Google is asked to make them type their password again;
  GitHub has no way to ask that, so with GitHub it only proves the account
  is still theirs.
- Signing in is rate limited per address. Behind a reverse proxy everyone
  comes from the proxy's address and shares one limit.
- Turning an account off at Google or GitHub does not remove the member.
  Remove them in Wenlan.

If a member's Google or GitHub account was taken over, the owner chooses
**Account taken over…** in that member's menu: every browser session and AI
tool key of theirs stops, their linked accounts are dropped, and the owner
gets a new invite link to send them, so they can start again.

## Join

- **Desktop app**: in Settings › Team, choose "Join with an invite link" and paste it:
  one "Join Acme" joins every Space opened to you, and Spaces opened to you
  later follow by themselves. From a terminal,
  `wenlan team join <invite link>` does the same: every Space open to you is
  synced to this computer. A Space of your own with the same name is left
  alone, and the team's copy goes beside it as `Product (Acme)`.
  `wenlan team join <invite link> --list` only shows what the link opens,
  `--server-space Product --as <name>` takes one Space under another name,
  `wenlan team status` shows the teams this computer joined, and
  `wenlan team leave` leaves one (the pages already on the computer stay).
- **An AI tool, with sign-in on**: in the AI tool's terminal, run

  ```bash
  wenlan login https://wenlan.example.com --tool claude-code   # or codex, cursor
  ```

  It prints a short code and the address to type it at,
  `https://wenlan.example.com/app/device`. Open it, type the code, check what the tool asks for, and choose **Allow**.
  Wenlan then writes the tool's MCP settings itself (`~/.claude.json`,
  `~/.codex/config.toml` or `~/.cursor/mcp.json`) with a key for that tool
  alone, kept in a file only you can read. Each tool has its own key: see or
  remove them from the account menu (**AI tools**), or with
  `wenlan team keys` and `wenlan team revoke-key <id>`. A removed key stops
  at its next request. The owner sees how many AI tools each member has and
  can remove them.
- **AI tool only, with an invite link**: add a remote MCP server with URL
  `https://wenlan.example.com/mcp` (or `/mcp/<Space>` when you have more than
  one Space) and header `Authorization: Bearer wlt_…`. Members who may write
  get the web-write tools (search, read, write, rename and delete pages,
  capture); read-only members get search only (`brief`, `recall`,
  `get_page_sources`). To change an existing page, the AI tool must pass the
  version it last read, so it never writes over a teammate's newer edit.

## Limits in this version

- Not real-time co-editing: changes arrive within about 30 seconds. Two
  people editing the same page at the same time: the later edit is shown,
  and the other stays in the page's history with a note to its writer.
- Only pages are shared. Memories, sources and other Spaces stay on each
  computer.
- A read-only member can still type in their own copy; those edits are not
  sent, and the team's version replaces them on the next sync.
