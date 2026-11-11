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
| `GET /team/v1/me` | who the key belongs to and their Spaces |
| `GET /team/v1/spaces/{space}/files`, `…/file?path=` | the Space's page versions |
| `PUT /team/v1/spaces/{space}/versions/{page}/{sha}.wlv` | the member's own new version |
| `/mcp`, `/mcp/{space}` | MCP for AI tools, pinned to that Space |

Every request except `/health` needs `Authorization: Bearer <member key>`.
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
  and use `https://wenlan.example.com`.

A member's Wenlan refuses a plain `http://` address unless it is inside a
private network (this computer, a LAN address, `.local`, Tailscale).

## Set up the server

1. Run Wenlan on the server with the team listener on:

   ```bash
   docker run -d --name wenlan \
     -p 127.0.0.1:7878:7878 -p 7879:7879 \
     -e WENLAN_TEAM_ADDR=0.0.0.0:7879 \
     -v wenlan-data:/data wenlan-server
   ```

   Keep the daemon's own port (7878) on loopback, as the Docker notes say.
   Only 7879 is for the team. On a desktop install, set `WENLAN_TEAM_ADDR`
   for the background service instead.

2. Host a Space and add people (on the server):

   ```bash
   wenlan team host "Product"
   wenlan team add "Amy" --access Product --url https://wenlan.example.com
   wenlan team add "Bo"  --access Product:read --url https://wenlan.example.com
   ```

   Each `add` prints a join code once, such as
   `https://wenlan.example.com#wlt_…`. Send it to that person privately.
   `wenlan team status` lists members; `wenlan team remove <id>` removes one;
   `wenlan team new-key <id>` replaces a lost key.

## Join

- **Desktop app**: `wenlan team join <join code>` lists the Spaces you can
  use; `wenlan team join <join code> --server-space Product` adds one
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
