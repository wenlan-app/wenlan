// SPDX-License-Identifier: Apache-2.0
//! A member's Wenlan follows their team: every Space the team server lets
//! them use is synced here without joining it Space by Space, including
//! Spaces opened to them later (a Space for everyone, or one they were
//! added to). A Space they stopped syncing here stays stopped until they
//! join it again by hand.
//!
//! A followed Space is never merged into a Space of the same name that is
//! already here (it may hold this person's own pages): it is made under
//! `<name> (<server>)` instead.

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
    let hosts: BTreeSet<(String, String)> = shares
        .iter()
        .filter_map(|(_, settings)| settings.host.as_ref())
        .map(|host| (host.url.clone(), host.key.clone()))
        .collect();
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
        let Some(local) = local_space(db, settings_path, url, remote_space).await? else {
            continue;
        };
        let host = HostLink {
            url: url.to_string(),
            key: key.to_string(),
            space: remote_space.to_string(),
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
) -> Result<Option<String>, WenlanError> {
    let store = super::read_settings(settings_path)?;
    if followed(&store.spaces, url, remote_space)
        || store.left.contains(&left_key(url, remote_space))
    {
        return Ok(None);
    }
    let server = reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string());
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
    use crate::shared_space::Scope;

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
}
