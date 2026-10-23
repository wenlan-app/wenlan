# Glama listing

Wenlan's repository is `wenlan-app/wenlan`. Its root `glama.json` names the
GitHub account allowed to maintain the Glama listing for this organization-owned
repository.

## Submit and inspect

1. Sign in to [Glama](https://glama.ai/mcp/servers) with the maintainer's GitHub
   account and submit `https://github.com/wenlan-app/wenlan` through **Add Server**.
2. Claim the listing after `glama.json` is available on the default branch.
3. Paste [`docker/Dockerfile.glama`](../docker/Dockerfile.glama) directly into
   Glama's Dockerfile admin page and deploy the build for inspection.
4. Confirm Glama reports a successful inspection and detects the MCP tools.
   If the current Glama flow requires a release, make one from the successful
   build before checking the quality score.
5. Copy the score badge from the actual listing into the awesome-mcp-servers
   entry, after its description. Use the path Glama assigns to this listing.

The image runs the real published `wenlan-mcp` 0.19.3 binary over stdio as a
non-root user. `initialize` and `tools/list` are served locally, without API keys
or a daemon. The default origin is an unused container-loopback port, so the
inspection does not connect to a user's installed daemon. No host directories,
wiki pages, credentials, or ports need to be mounted or exposed.

This image establishes MCP discovery only. Tool calls that read or write wiki
data require a reachable Wenlan daemon. For those calls, pass an explicit origin,
for example `docker run --rm -i IMAGE --origin-url http://DAEMON:7878`, using a
daemon reachable from the container and its required access controls.

The MCP process also performs a best-effort release update check; its cache is
confined to `/tmp/wenlan-mcp-cache` inside the container.

## Local check

Build from the repository root; the Dockerfile-specific ignore file excludes
the checkout from the build context:

```sh
docker build -f docker/Dockerfile.glama -t wenlan-mcp:glama .
```

Connect an MCP client to `docker run --rm -i --network none wenlan-mcp:glama`.
Send `initialize`, then `notifications/initialized`, then `tools/list`; require
successful JSON-RPC responses for both requests and a nonempty tool list.
Disabling networking here proves discovery does not need a daemon or remote
service. This local result does not establish that Glama's own build and
inspection have passed.

When updating the pinned npm package, rebuild the image and repeat this check.

Glama references: [claiming a server](https://glama.ai/blog/2025-07-08-what-is-glamajson),
[metadata schema](https://glama.ai/mcp/schemas/server.json).
