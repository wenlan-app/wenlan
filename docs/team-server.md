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
- **AI-tool-only members** (no Wenlan installed) point their AI tool at the
  server's `/mcp` with their key. What their AI tool changes is recorded as
  theirs ("Cy's AI tool").
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

Every request except `/health` and `/join` needs `Authorization: Bearer <member key>`.
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

### From a terminal

On the server itself, the same can be done with commands:

```bash
wenlan team rename "Acme"
wenlan team host "Product"
wenlan team add "Amy" --access Product --url https://wenlan.example.com
wenlan team add "Bo"  --access Product:read --url https://wenlan.example.com
```

Each `add` prints an invite link once, such as
`https://wenlan.example.com/join#wlt_…`. Send it to that person privately.
The key stays after `#`, so a browser opening the link never sends it to the
server. Older links without `/join` still work. Renaming a hosted Space
renames it for the whole team: access goes with it, and members' copies
follow by themselves. `wenlan team status` lists
members; `wenlan team remove <id>` removes one; `wenlan team new-key <id>`
replaces a lost key. A desktop install can also set `WENLAN_TEAM_ADDR` for
the background service instead of starting the team in Settings.

## Join

- **Desktop app**: in Settings › Team, choose "Join with an invite link" and paste it:
  one "Join Acme" joins every Space opened to you, and Spaces opened to you
  later follow by themselves. From a terminal,
  `wenlan team join <invite link>` lists the Spaces you can
  use; `wenlan team join <invite link> --server-space Product` adds one
  (`--as <name>` gives it another name on your computer).
- **AI tool only**: add a remote MCP server with URL
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
