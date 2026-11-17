// SPDX-License-Identifier: AGPL-3.0-only
// Who may do what on a team server. The server asks these rules before
// every team call; the facts (who is the owner, which Spaces a member was
// given, which Spaces this server hosts) come from the team file and the
// hosted Spaces, never from the request.

// The owner may read and edit every Space this server hosts.
@id("owner-uses-every-hosted-space")
permit (
    principal is Wenlan::Member,
    action in [Wenlan::Action::"read", Wenlan::Action::"write"],
    resource is Wenlan::Space
)
when { principal.owner && resource.hosted };

// A member may read a hosted Space they were given, to read or to edit.
@id("member-reads-given-space")
permit (
    principal is Wenlan::Member,
    action == Wenlan::Action::"read",
    resource is Wenlan::Space
)
when {
    resource.hosted &&
    (principal.reads.contains(resource) || principal.writes.contains(resource))
};

// A member may edit a hosted Space they were given to edit.
@id("member-edits-given-space")
permit (
    principal is Wenlan::Member,
    action == Wenlan::Action::"write",
    resource is Wenlan::Space
)
when { resource.hosted && principal.writes.contains(resource) };

// Only the owner manages the team (members, invites, hosted Spaces).
@id("owner-manages-team")
permit (
    principal is Wenlan::Member,
    action == Wenlan::Action::"manage",
    resource is Wenlan::Team
)
when { principal.owner };

// Someone removed from the team may do nothing, whatever else says yes.
@id("removed-member-does-nothing")
forbid (principal is Wenlan::Member, action, resource)
unless { principal.active };
