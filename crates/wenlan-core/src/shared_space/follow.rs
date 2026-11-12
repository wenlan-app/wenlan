// SPDX-License-Identifier: Apache-2.0
//! A member's Wenlan follows their team: every Space the team server lets
//! them use is synced here without joining it Space by Space, including
//! Spaces opened to them later (a Space for everyone, or one they were
//! added to). A Space they stopped syncing here stays stopped until they
//! join it again by hand.
//!
//! A followed Space is never merged into a Space of the same name that is
//! already here (it may hold this person's own pages): it is made under
//! `<name> (<team>)` instead (the server's host until the team is named).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use wenlan_types::team::TeamMe;

use super::{change_settings, left_key, remember_team, remote, shares_at, HostLink, ShareSettings};
use crate::db::MemoryDB;
use crate::error::WenlanError;

/// Follows every team server this Wenlan already syncs a Space through.
/// A server that cannot be reached is skipped until the next pass.
pub(super) async fn follow_teams(db: &MemoryDB, settings_path: &Path) {
    let shares = match shares_at(settings_path) {
        Ok(shares) => shares,
        Err(_) => return,
    };
    let mut hosts: BTreeSet<(String, String)> = shares
        .iter()
        .filter_map(|(_, settings)| settings.host.as_ref())
        .map(|host| (host.url.clone(), host.key.clone()))
        .collect();
    // A team joined before any Space was open to this person.
    if let Ok(memberships) = super::memberships_at(settings_path) {
        hosts.extend(
            memberships
                .into_iter()
                .map(|(url, membership)| (url, membership.key)),
        );
    }
    for (url, key) in hosts {
        let me = match remote::me(&url, &key).await {
            Ok(me) => me,
            Err(reason) => {
                log::debug!("[shared_space] team {url}: not followed this time: {reason}");
                continue;
            }
        };
        if let Some(team) = &me.team {
            if let Err(error) = remember_team(settings_path, &url, team) {
                log::warn!("[shared_space] team {url}: name not kept: {error}");
            }
        }
        // Who they are on it, the owner or not, as the server says now.
        if let Err(error) = super::remember_membership(settings_path, &url, &key, &me) {
            log::warn!("[shared_space] team {url}: membership not kept: {error}");
        }
        if let Err(error) = follow_renames(db, settings_path, &url, &me).await {
            log::warn!("[shared_space] team {url}: a renamed Space not followed: {error}");
        }
        match follow_host(db, settings_path, &url, &key, &me).await {
            Ok(added) => {
                for space in added {
                    log::info!("[shared_space] team {url}: now syncing {space}");
                }
            }
            Err(error) => log::warn!("[shared_space] team {url}: not followed: {error}"),
        }
    }
}

/// Starts syncing here each Space `me` may use on the server at `url` that
/// is not synced here yet and was not left. The next pass pulls it.
/// Returns the local names of the Spaces it added.
pub(super) async fn follow_host(
    db: &MemoryDB,
    settings_path: &Path,
    url: &str,
    key: &str,
    me: &TeamMe,
) -> Result<Vec<String>, WenlanError> {
    let mut added = Vec::new();
    for entry in &me.spaces {
        let remote_space = entry.space.as_str();
        let team = me.team.as_ref().and_then(|team| team.name.as_deref());
        let Some(local) = local_space(db, settings_path, url, remote_space, team).await? else {
            continue;
        };
        let host = HostLink {
            url: url.to_string(),
            key: key.to_string(),
            space: remote_space.to_string(),
            space_id: entry.id.clone(),
        };
        let folder = remote::mirror_folder(settings_path, url, remote_space)
            .to_string_lossy()
            .into_owned();
        let settings = ShareSettings {
            member_id: me.member_id.clone(),
            member_name: me.name.clone(),
            folder,
            host: Some(host),
            read_only: entry.access == wenlan_types::team::TeamAccess::Read,
            hosted: false,
        };
        let joined = change_settings(settings_path, |store| {
            // Checked again under the lock: a join by hand may have won.
            if followed(&store.spaces, url, remote_space)
                || store.left.contains(&left_key(url, remote_space))
                || store.spaces.contains_key(&local)
            {
                return (false, false);
            }
            store.spaces.insert(local.clone(), settings);
            (true, true)
        })?;
        if joined {
            added.push(local);
        }
    }
    Ok(added)
}

/// A Space renamed on the server keeps its id there: the share here
/// follows it to the new name, and so does the Space here when it carried
/// the server's name (`<old>` or `<old> (<team>)`; a name this person chose
/// stays theirs, as does a name another Space here already has).
pub(super) async fn follow_renames(
    db: &MemoryDB,
    settings_path: &Path,
    url: &str,
    me: &TeamMe,
) -> Result<(), WenlanError> {
    for entry in &me.spaces {
        let Some(id) = entry.id.as_deref() else {
            continue;
        };
        // Under the lock: which share here is this Space, and the name it
        // had on the server. A share from before ids were sent learns its id.
        let renamed = change_settings(settings_path, |store| {
            let found = store.spaces.iter_mut().find(|(_, settings)| {
                settings.host.as_ref().is_some_and(|host| {
                    host.url == url
                        && match host.space_id.as_deref() {
                            Some(known) => known == id,
                            None => host.space == entry.space,
                        }
                })
            });
            let Some((local, settings)) = found else {
                return (false, None);
            };
            let Some(host) = settings.host.as_mut() else {
                return (false, None);
            };
            let before = host.clone();
            host.space_id = Some(id.to_string());
            let old = std::mem::replace(&mut host.space, entry.space.clone());
            let changed = *host != before;
            (changed, (old != entry.space).then(|| (local.clone(), old)))
        })?;
        let Some((local, old)) = renamed else {
            continue;
        };
        log::info!("[shared_space] team {url}: {old} is now {}", entry.space);
        let new_local = if local == old {
            entry.space.clone()
        } else if let Some(suffix) = local
            .strip_prefix(&old)
            .filter(|rest| rest.starts_with(" ("))
        {
            format!("{}{suffix}", entry.space)
        } else {
            continue;
        };
        if db
            .registered_space_or_none(Some(&new_local))
            .await?
            .is_some()
        {
            continue;
        }
        let description = db
            .get_space(&local)
            .await?
            .and_then(|space| space.description);
        db.update_space(&local, &new_local, description.as_deref())
            .await?;
        super::rename_share(settings_path, &local, &new_local)?;
    }
    Ok(())
}

fn followed(spaces: &BTreeMap<String, ShareSettings>, url: &str, remote_space: &str) -> bool {
    spaces.values().any(|settings| {
        settings
            .host
            .as_ref()
            .is_some_and(|host| host.url == url && host.space == remote_space)
    })
}

/// The Space here that `remote_space` syncs into, made when needed; `None`
/// when it is synced here already, was left, or no free name is found.
async fn local_space(
    db: &MemoryDB,
    settings_path: &Path,
    url: &str,
    remote_space: &str,
    team: Option<&str>,
) -> Result<Option<String>, WenlanError> {
    let store = super::read_settings(settings_path)?;
    if followed(&store.spaces, url, remote_space)
        || store.left.contains(&left_key(url, remote_space))
    {
        return Ok(None);
    }
    let server = match team.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => name.to_string(),
        None => reqwest::Url::parse(url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_string))
            .unwrap_or_else(|| url.to_string()),
    };
    let suffixed = format!("{remote_space} ({server})");
    // The plain name only when no Space here has it; the suffixed one also
    // when it is here but shared with nothing (made by an earlier follow).
    if db
        .registered_space_or_none(Some(remote_space))
        .await?
        .is_none()
    {
        db.create_space(remote_space, None, false).await?;
        return db.registered_space_or_none(Some(remote_space)).await;
    }
    match db.registered_space_or_none(Some(&suffixed)).await? {
        None => {
            db.create_space(&suffixed, None, false).await?;
            db.registered_space_or_none(Some(&suffixed)).await
        }
        Some(existing) if !store.spaces.contains_key(&existing) => Ok(Some(existing)),
        Some(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use wenlan_types::team::{TeamAccess, TeamSpaceAccess};

    use super::*;
    use crate::events::NoopEmitter;
    use crate::shared_space::{share_settings_at, Scope};

    const URL: &str = "https://team.example.com";

    fn me(spaces: &[(&str, TeamAccess)]) -> TeamMe {
        TeamMe {
            member_id: "m_amy".into(),
            name: "Amy".into(),
            spaces: spaces
                .iter()
                .map(|(space, access)| TeamSpaceAccess {
                    space: space.to_string(),
                    access: *access,
                    id: None,
                })
                .collect(),
            team: None,
            role: Default::default(),
        }
    }

    #[tokio::test]
    async fn every_team_space_is_synced_here_and_a_left_one_stays_left() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join(super::super::SETTINGS_FILE);
        let db = MemoryDB::new(&dir.path().join("wenlan.db"), Arc::new(NoopEmitter))
            .await
            .unwrap();
        // A personal Space with a team Space's name is left alone.
        db.create_space("Product", None, false).await.unwrap();

        let first = me(&[("Product", TeamAccess::Write), ("Design", TeamAccess::Read)]);
        let added = follow_host(&db, &settings, URL, "wlt_k", &first)
            .await
            .unwrap();
        assert_eq!(added, vec!["Product (team.example.com)", "Design"]);
        let shares: BTreeMap<_, _> = shares_at(&settings)
            .unwrap()
            .into_iter()
            .map(|(scope, settings)| (scope.key(), settings))
            .collect();
        assert!(
            !shares.contains_key("Product"),
            "the personal Space is not shared"
        );
        let design = &shares["Design"];
        assert_eq!(design.member_id, "m_amy");
        assert!(design.read_only);
        assert_eq!(design.host.as_ref().unwrap().space, "Design");
        assert_eq!(
            shares["Product (team.example.com)"]
                .host
                .as_ref()
                .unwrap()
                .space,
            "Product"
        );

        // Nothing new: nothing added. A Space opened later is added.
        assert!(follow_host(&db, &settings, URL, "wlt_k", &first)
            .await
            .unwrap()
            .is_empty());
        super::super::leave_at(&settings, &Scope::Space("Design".into())).unwrap();
        let later = me(&[
            ("Product", TeamAccess::Write),
            ("Design", TeamAccess::Read),
            ("Sales", TeamAccess::Write),
        ]);
        let added = follow_host(&db, &settings, URL, "wlt_k", &later)
            .await
            .unwrap();
        assert_eq!(
            added,
            vec!["Sales"],
            "a Space left here is not followed again"
        );

        // Joining it again by hand (which syncs it at once) clears that.
        super::super::rejoined(&settings, URL, "Design").unwrap();
        assert!(super::super::read_settings(&settings)
            .unwrap()
            .left
            .is_empty());
    }

    fn with_ids(me: TeamMe, ids: &[&str]) -> TeamMe {
        let spaces = me
            .spaces
            .into_iter()
            .zip(ids)
            .map(|(entry, id)| TeamSpaceAccess {
                id: Some(id.to_string()),
                ..entry
            })
            .collect();
        TeamMe { spaces, ..me }
    }

    #[tokio::test]
    async fn a_space_renamed_on_the_server_is_followed_by_its_id() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join(super::super::SETTINGS_FILE);
        let db = MemoryDB::new(&dir.path().join("wenlan.db"), Arc::new(NoopEmitter))
            .await
            .unwrap();
        // A personal Space named like the team's makes the team's suffixed.
        db.create_space("Design", None, false).await.unwrap();
        // Followed before the server sent ids.
        let before = me(&[
            ("Product", TeamAccess::Write),
            ("Design", TeamAccess::Write),
        ]);
        follow_host(&db, &settings, URL, "wlt_k", &before)
            .await
            .unwrap();
        db.update_space("Product", "Product", Some("kept"))
            .await
            .unwrap();
        let host_of = |name: &str| {
            share_settings_at(&settings, &Scope::Space(name.into()))
                .unwrap()
                .and_then(|share| share.host)
        };

        // The same names, now with ids: each share learns its id.
        let ids = with_ids(before.clone(), &["s1", "s2"]);
        follow_renames(&db, &settings, URL, &ids).await.unwrap();
        assert_eq!(host_of("Product").unwrap().space_id.as_deref(), Some("s1"));

        // Renamed on the server: the share and the Space here follow, and
        // follow_host adds nothing new.
        let renamed = with_ids(
            me(&[
                ("Roadmap", TeamAccess::Write),
                ("Visual", TeamAccess::Write),
            ]),
            &["s1", "s2"],
        );
        follow_renames(&db, &settings, URL, &renamed).await.unwrap();
        assert!(follow_host(&db, &settings, URL, "wlt_k", &renamed)
            .await
            .unwrap()
            .is_empty());
        assert!(host_of("Product").is_none());
        assert_eq!(host_of("Roadmap").unwrap().space, "Roadmap");
        let roadmap = db.get_space("Roadmap").await.unwrap().unwrap();
        assert_eq!(roadmap.description.as_deref(), Some("kept"));
        assert!(db.get_space("Product").await.unwrap().is_none());
        assert_eq!(
            host_of("Visual (team.example.com)").unwrap().space,
            "Visual"
        );
        assert!(
            db.get_space("Design").await.unwrap().is_some(),
            "personal stays"
        );

        // A name this person chose here stays theirs; the share follows.
        db.update_space("Roadmap", "Our plans", None).await.unwrap();
        super::super::rename_share(&settings, "Roadmap", "Our plans").unwrap();
        let again = with_ids(
            me(&[("Plans", TeamAccess::Write), ("Visual", TeamAccess::Write)]),
            &["s1", "s2"],
        );
        follow_renames(&db, &settings, URL, &again).await.unwrap();
        assert_eq!(host_of("Our plans").unwrap().space, "Plans");
    }

    #[tokio::test]
    async fn a_team_space_carries_the_team_its_server_named() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join(super::super::SETTINGS_FILE);
        let db = MemoryDB::new(&dir.path().join("wenlan.db"), Arc::new(NoopEmitter))
            .await
            .unwrap();
        follow_host(
            &db,
            &settings,
            URL,
            "wlt_k",
            &me(&[("Design", TeamAccess::Write)]),
        )
        .await
        .unwrap();
        let team_of = |settings: &Path| {
            super::super::share_statuses_at(settings).unwrap()[0]
                .team
                .clone()
        };
        assert_eq!(team_of(&settings), None, "until the server says");
        let acme = wenlan_types::team::TeamInfo {
            id: "t1".into(),
            name: Some("Acme".into()),
        };
        remember_team(&settings, URL, &acme).unwrap();
        assert_eq!(team_of(&settings), Some(acme.clone()));
        let status = super::super::share_status_at(&settings, &Scope::Space("Design".into()))
            .unwrap()
            .unwrap();
        assert_eq!(status.team, Some(acme));
    }

    #[test]
    fn a_team_is_kept_from_its_invite_link_and_its_owner_is_known() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join(super::super::SETTINGS_FILE);
        assert_eq!(super::super::owned_team(&settings).unwrap(), None);

        // Joined with nothing open yet: the team is still kept.
        let member = me(&[]);
        super::super::remember_membership(&settings, URL, "wlt_k", &member).unwrap();
        let kept = super::super::memberships_at(&settings).unwrap();
        assert_eq!(kept[URL].member_id, "m_amy");
        assert!(!kept[URL].owner);
        assert_eq!(super::super::owned_team(&settings).unwrap(), None);

        // The server says this person owns it now.
        let owner = TeamMe {
            role: wenlan_types::team::TeamRole::Owner,
            ..me(&[])
        };
        super::super::remember_membership(&settings, URL, "wlt_k", &owner).unwrap();
        assert_eq!(
            super::super::owned_team(&settings).unwrap(),
            Some((URL.to_string(), "wlt_k".to_string()))
        );

        super::super::forget_membership(&settings, URL).unwrap();
        assert!(super::super::memberships_at(&settings).unwrap().is_empty());
        assert_eq!(super::super::owned_team(&settings).unwrap(), None);
    }
}
