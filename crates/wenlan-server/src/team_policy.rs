// SPDX-License-Identifier: AGPL-3.0-only
//! Who may do what on a team server, decided by Cedar.
//!
//! The rules are in `team_policy/policy.cedar`, checked against
//! `team_policy/schema.cedarschema` when they are loaded. This module only
//! turns the facts the server already has (the member from the team file,
//! the Spaces this server hosts) into Cedar entities and asks.
//!
//! Who someone is (the member key) is decided before this, in
//! `require_member`; what is taken out of an answer stays with the gate.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::str::FromStr;
use std::sync::OnceLock;

use cedar_policy::{
    Authorizer, Context, Decision, Entities, Entity, EntityId, EntityTypeName, EntityUid,
    PolicySet, Request, RestrictedExpression, Schema,
};
use wenlan_core::team::Member;
use wenlan_types::team::{TeamAccess, TeamRole};

const POLICY: &str = include_str!("team_policy/policy.cedar");
const SCHEMA: &str = include_str!("team_policy/schema.cedarschema");

/// What a member asks to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// Read a Space's pages.
    Read,
    /// Change a Space's pages.
    Write,
    /// Manage the team itself: members, invites, hosted Spaces.
    Manage,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Action::Read => "read",
            Action::Write => "write",
            Action::Manage => "manage",
        }
    }
}

struct Rules {
    policies: PolicySet,
    schema: Schema,
}

/// The rules, read and checked once. The files are part of the program, so
/// a rule that does not load is a bug caught by the tests, never a state a
/// running server reaches; if it ever did, every question answers no.
fn rules() -> Option<&'static Rules> {
    static RULES: OnceLock<Option<Rules>> = OnceLock::new();
    RULES
        .get_or_init(|| {
            let loaded = load();
            if let Err(error) = &loaded {
                tracing::error!("[team] the permission rules do not load: {error}");
            }
            loaded.ok()
        })
        .as_ref()
}

fn load() -> Result<Rules, String> {
    let (schema, _) = Schema::from_cedarschema_str(SCHEMA).map_err(|e| e.to_string())?;
    let policies = PolicySet::from_str(POLICY).map_err(|e| e.to_string())?;
    let validation = cedar_policy::Validator::new(schema.clone())
        .validate(&policies, cedar_policy::ValidationMode::Strict);
    if !validation.validation_passed() {
        let errors: Vec<String> = validation
            .validation_errors()
            .map(|e| e.to_string())
            .collect();
        return Err(errors.join("; "));
    }
    Ok(Rules { policies, schema })
}

fn uid(kind: &str, id: &str) -> Option<EntityUid> {
    let kind = EntityTypeName::from_str(kind).ok()?;
    Some(EntityUid::from_type_name_and_id(kind, EntityId::new(id)))
}

fn space_uid(space: &str) -> Option<EntityUid> {
    uid("Wenlan::Space", space)
}

fn set(spaces: &BTreeSet<String>) -> RestrictedExpression {
    RestrictedExpression::new_set(
        spaces
            .iter()
            .filter_map(|space| space_uid(space))
            .map(RestrictedExpression::new_entity_uid),
    )
}

/// The Spaces `member` was given, to read and to edit: their own grants and
/// the ones given to everyone, their own winning.
fn grants(member: &Member) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut all = member.everyone.clone();
    all.extend(member.spaces.clone());
    let mut reads = BTreeSet::new();
    let mut writes = BTreeSet::new();
    for (space, access) in all {
        match access {
            TeamAccess::Read => reads.insert(space),
            TeamAccess::Write => writes.insert(space),
        };
    }
    (reads, writes)
}

/// Whether `member` may do `action`: on the Space `space` (read, write),
/// or on the team (manage). `hosted` says whether this server hosts
/// `space`. Anything that cannot be asked answers no.
pub(crate) fn allowed(member: &Member, action: Action, space: Option<&str>, hosted: bool) -> bool {
    ask(member, action, space, hosted).unwrap_or(false)
}

fn ask(member: &Member, action: Action, space: Option<&str>, hosted: bool) -> Option<bool> {
    let rules = rules()?;
    let principal = uid("Wenlan::Member", &member.id)?;
    let (reads, writes) = grants(member);
    let mut entities = vec![Entity::new(
        principal.clone(),
        HashMap::from([
            (
                "owner".to_string(),
                RestrictedExpression::new_bool(member.role == TeamRole::Owner),
            ),
            (
                "active".to_string(),
                RestrictedExpression::new_bool(member.is_active()),
            ),
            ("reads".to_string(), set(&reads)),
            ("writes".to_string(), set(&writes)),
        ]),
        HashSet::new(),
    )
    .ok()?];
    // Every Space the member names is an entity, so the sets above refer
    // to known things; only the one asked about can be hosted.
    for name in reads.iter().chain(writes.iter()) {
        if Some(name.as_str()) != space {
            entities.push(
                Entity::new(
                    space_uid(name)?,
                    HashMap::from([("hosted".to_string(), RestrictedExpression::new_bool(false))]),
                    HashSet::new(),
                )
                .ok()?,
            );
        }
    }
    let resource = match action {
        Action::Manage => {
            let team = uid("Wenlan::Team", "team")?;
            entities.push(Entity::new_no_attrs(team.clone(), HashSet::new()));
            team
        }
        Action::Read | Action::Write => {
            let space = space_uid(space?)?;
            entities.push(
                Entity::new(
                    space.clone(),
                    HashMap::from([("hosted".to_string(), RestrictedExpression::new_bool(hosted))]),
                    HashSet::new(),
                )
                .ok()?,
            );
            space
        }
    };
    let entities = Entities::from_entities(entities, Some(&rules.schema)).ok()?;
    let request = Request::new(
        principal,
        uid("Wenlan::Action", action.name())?,
        resource,
        Context::empty(),
        Some(&rules.schema),
    )
    .ok()?;
    let answer = Authorizer::new().is_authorized(&request, &rules.policies, &entities);
    Some(answer.decision() == Decision::Allow)
}

/// What `member` may do in `space` on this server, by the rules: `None`
/// when they may not even read it.
pub(crate) fn access(member: &Member, space: &str, hosted: bool) -> Option<TeamAccess> {
    if allowed(member, Action::Write, Some(space), hosted) {
        Some(TeamAccess::Write)
    } else if allowed(member, Action::Read, Some(space), hosted) {
        Some(TeamAccess::Read)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "team_policy_test.rs"]
mod tests;
