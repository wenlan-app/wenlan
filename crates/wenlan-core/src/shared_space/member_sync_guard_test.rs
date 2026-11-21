//! Structural drift guard for the hosted member sync write boundary.
//!
//! These checks intentionally pin the route and write seams that make member
//! imports safe. They are not a substitute for the runtime proofs recorded by
//! the round 7 task owner.

#![cfg(test)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use syn::visit::{self, Visit};
use syn::{Expr, ExprMethodCall, File, ImplItem, Item, ItemImpl};

fn workspace_root() -> PathBuf {
    if let Some(root) = std::env::var_os("ROUND7_WORKSPACE_ROOT") {
        return PathBuf::from(root);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn source(relative: &str) -> String {
    fs::read_to_string(workspace_root().join(relative)).unwrap()
}

fn parsed(relative: &str) -> File {
    syn::parse_file(&source(relative))
        .unwrap_or_else(|error| panic!("{relative} stopped parsing as Rust: {error}"))
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Call {
    method: String,
    receiver: String,
}

fn expr_shape(expr: &Expr) -> String {
    match expr {
        Expr::Path(path) => path
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        Expr::Field(field) => {
            let member = match &field.member {
                syn::Member::Named(ident) => ident.to_string(),
                syn::Member::Unnamed(index) => index.index.to_string(),
            };
            format!("{}.{}", expr_shape(&field.base), member)
        }
        Expr::MethodCall(call) => format!("{}()", call.method),
        Expr::Reference(reference) => format!("&{}", expr_shape(&reference.expr)),
        Expr::Paren(paren) => expr_shape(&paren.expr),
        _ => "<complex-receiver>".to_string(),
    }
}

#[derive(Default)]
struct Calls(Vec<Call>);

impl<'ast> Visit<'ast> for Calls {
    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        self.0.push(Call {
            method: node.method.to_string(),
            receiver: expr_shape(&node.receiver),
        });
        visit::visit_expr_method_call(self, node);
    }
}

fn all_calls(file: &File) -> Vec<Call> {
    let mut calls = Calls::default();
    calls.visit_file(file);
    calls.0.sort();
    calls.0
}

fn method_calls(item: &ItemImpl, method: &str) -> Vec<Call> {
    let function = item
        .items
        .iter()
        .find_map(|item| match item {
            ImplItem::Fn(function) if function.sig.ident == method => Some(function),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "missing method {method}; found {:?}",
                item.items
                    .iter()
                    .filter_map(|item| match item {
                        ImplItem::Fn(function) => Some(function.sig.ident.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            )
        });
    block_calls(&function.block)
}

fn block_calls(block: &syn::Block) -> Vec<Call> {
    let mut calls = Calls::default();
    calls.visit_block(block);
    calls.0.sort();
    calls.0
}

fn impl_for<'a>(file: &'a File, type_name: &str) -> &'a ItemImpl {
    file.items
        .iter()
        .find_map(|item| match item {
            Item::Impl(item)
                if matches!(item.self_ty.as_ref(), syn::Type::Path(ty)
                if ty.path.segments.last().is_some_and(|segment| segment.ident == type_name)) =>
            {
                Some(item)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing impl for {type_name}"))
}

fn call_set(calls: &[Call]) -> BTreeSet<Call> {
    calls.iter().cloned().collect()
}

fn has_call(calls: &[Call], method: &str, receiver: &str) -> bool {
    calls
        .iter()
        .any(|call| call.method == method && call.receiver == receiver)
}

/// The import module may orchestrate normal reads and root export, but its
/// page/card mutation seam is the typed member writer alone. Keep the full
/// per-route mutator inventory explicit so new DB-style mutators fail closed.
fn audit_import_mutators(file: &File) -> Result<(), String> {
    let calls = all_calls(file);
    let forbidden: BTreeSet<&str> = [
        "apply_version",
        "write_merged",
        "keep_as_second_page",
        "mark_concurrent_edit",
        "set_page",
        "set_page_citations",
        "set_page_review_required",
        "archive_page",
        "delete_page",
        "move_page",
        "insert_page",
        "apply_page",
        "stage_card",
        "close_card",
        "decide",
        "decide_team_rules",
    ]
    .into_iter()
    .collect();
    if let Some(call) = calls
        .iter()
        .find(|call| forbidden.contains(call.method.as_str()))
    {
        return Err(format!(
            "member import added unapproved mutator {} on {}",
            call.method, call.receiver
        ));
    }
    let self_methods: BTreeSet<&str> = ["member_writer", "export_page", "reload_hosted"]
        .into_iter()
        .collect();
    if let Some(call) = calls
        .iter()
        .find(|call| call.receiver == "self" && !self_methods.contains(call.method.as_str()))
    {
        return Err(format!(
            "member import added an unreviewed self call {}",
            call.method
        ));
    }
    if let Some(call) = calls.iter().find(|call| {
        call.receiver.starts_with("self.")
            && !(matches!(call.receiver.as_str(), "self.refused" | "self.published")
                && call.method == "contains")
    }) {
        return Err(format!(
            "member import added an unreviewed field call {}.{}",
            call.receiver, call.method
        ));
    }
    if let Some(call) = calls.iter().find(|call| {
        (call.receiver == "writer" && call.method != "refuse")
            || (call.receiver == "member_writer()" && call.method != "write")
    }) {
        return Err(format!(
            "member import added an unreviewed writer call {} on {}",
            call.method, call.receiver
        ));
    }
    let writes = calls
        .iter()
        .filter(|call| call.method == "write" && call.receiver == "member_writer()")
        .count();
    let refusals = calls
        .iter()
        .filter(|call| call.method == "refuse" && call.receiver == "writer")
        .count();
    if writes != 2 || refusals != 1 {
        return Err(format!("expected two MemberSyncWriter writes and one refusal, found {writes} writes and {refusals} refusals"));
    }
    Ok(())
}

fn between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split_once(start)
        .unwrap_or_else(|| panic!("missing source anchor {start}"))
        .1
        .split_once(end)
        .unwrap_or_else(|| panic!("missing source boundary {end}"))
        .0
}

fn compact(source: &str) -> String {
    source
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn audit_db_calls(route: &str, calls: &[Call], expected: &[&str]) {
    let mut methods: Vec<_> = calls
        .iter()
        .filter(|call| matches!(call.receiver.as_str(), "self.db" | "db" | "tx" | "conn"))
        .map(|call| call.method.as_str())
        .collect();
    methods.sort_unstable();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    assert_eq!(
        methods, expected,
        "database call inventory changed in {route}"
    );
}

#[test]
fn hosted_pass_and_review_database_calls_match_the_guarded_inventory() {
    let sync = parsed("crates/wenlan-core/src/shared_space/mod.rs");
    let pass = sync
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(found) if found.sig.ident == "pass" => Some(found),
            _ => None,
        })
        .unwrap();
    audit_db_calls(
        "pass",
        &block_calls(&pass.block),
        &[
            "list_pages_by_space",
            "list_pages_by_space",
            "get_page",
            "personal_rule_page_ids",
            "shared_folder_pages",
            "shared_folder_pages",
            "bind_shared_deletion_archive",
            "set_app_metadata",
        ],
    );
    let pass_impl = impl_for(&sync, "Pass");
    audit_db_calls(
        "sync_page",
        &method_calls(pass_impl, "sync_page"),
        &[
            "shared_page_stay_token",
            "list_page_history",
            "mark_concurrent_edit",
        ],
    );
    audit_db_calls(
        "sync_missing",
        &method_calls(pass_impl, "sync_missing"),
        &["get_page"],
    );
    let review = parsed("crates/wenlan-core/src/shared_space/review.rs");
    let review_impl = impl_for(&review, "Pass");
    audit_db_calls(
        "review",
        &method_calls(review_impl, "review"),
        &[
            "get_app_metadata",
            "shared_page_stay_token",
            "get_shared_page_scoped",
            "page_card_state",
            "page_card_state",
            "open_shared_page_cards",
            "open_page_cards",
            "get_page",
            "get_page",
            "staged_card_body",
            "staged_card_body",
            "set_app_metadata",
        ],
    );
    audit_db_calls(
        "stage",
        &method_calls(review_impl, "stage"),
        &["get_page_source_revision"],
    );
}

fn hosted_dispatch_uses_writer(method: &str, entrypoint: &str) -> bool {
    let file = parsed("crates/wenlan-core/src/shared_space/mod.rs");
    let implementation = impl_for(&file, "Pass");
    has_call(&method_calls(implementation, method), entrypoint, "self")
}

#[test]
fn hosted_member_import_mutators_are_fail_closed_and_writer_only() {
    let file = parsed("crates/wenlan-core/src/shared_space/member_import.rs");
    audit_import_mutators(&file).expect("member import must stay behind MemberSyncWriter");

    let calls = all_calls(&file);
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.method == "write" && call.receiver == "member_writer()")
            .count(),
        2
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.method == "refuse" && call.receiver == "writer")
            .count(),
        1
    );
    assert!(calls
        .iter()
        .any(|call| call.method == "member_writer" && call.receiver == "self"));
}

#[test]
fn synthetic_unapproved_mutator_is_rejected_red_control() {
    // Causal RED control: substitute a direct legacy mutator for the typed
    // writer call in a synthetic route. This is never applied to product code.
    let bad = syn::parse_file("impl Pass { async fn apply_hosted_version(&self) { self.apply_version().await; } async fn apply_hosted_create(&self) { self.member_writer().write(input).await; } async fn refuse_hosted_changes(&self) { let writer = self.member_writer(); writer.refuse(change, reason).await; } }").unwrap();
    let rejection = audit_import_mutators(&bad).expect_err("the synthetic bypass must be rejected");
    eprintln!("RED control rejected synthetic direct mutator: {rejection}");
}

#[test]
fn hosted_dispatch_returns_before_legacy_edit_and_create_routes() {
    let file = parsed("crates/wenlan-core/src/shared_space/mod.rs");
    let implementation = impl_for(&file, "Pass");
    let page = method_calls(implementation, "sync_page");
    let create = method_calls(implementation, "sync_missing");

    assert!(has_call(&page, "apply_hosted_version", "self"));
    assert!(has_call(&page, "apply_version", "self"));
    assert!(has_call(&create, "apply_hosted_create", "self"));
    assert!(has_call(&create, "apply_create", "self"));

    let mod_source = source("crates/wenlan-core/src/shared_space/mod.rs");
    let page_route = mod_source
        .split("async fn sync_page(")
        .nth(1)
        .unwrap()
        .split("async fn sync_missing(")
        .next()
        .unwrap();
    let hosted_route = page_route
        .split("let together = match mine {")
        .nth(1)
        .unwrap()
        .split("// Versions written while this Wenlan was away")
        .next()
        .unwrap();
    assert!(
        compact(hosted_route).contains("returnself.apply_hosted_version("),
        "hosted edit route must return through writer before the legacy apply/merge/conflict path"
    );
    let missing_route = mod_source.split("async fn sync_missing(").nth(1).unwrap();
    let hosted_create = missing_route
        .split("if self.settings.hosted && !rulesync_team::is_master_page(id) {")
        .nth(1)
        .unwrap()
        .split("let Some(head) = version::head(versions)")
        .next()
        .unwrap();
    assert!(
        hosted_create.contains("return self.apply_hosted_create(versions, counts).await;"),
        "hosted create route must return through writer before legacy create path"
    );
}

#[test]
fn member_write_transaction_and_file_containment_are_bound_to_checked_route() {
    assert!(
        hosted_dispatch_uses_writer("sync_page", "apply_hosted_version"),
        "hosted edits must use the projection-checked transaction instead of the legacy writer"
    );
    let db_source = source("crates/wenlan-core/src/db/member_sync.rs");
    let write = between(
        &db_source,
        "pub(crate) async fn write(",
        "pub(crate) async fn refuse(",
    );
    let checked = between(
        &db_source,
        "async fn write_checked(",
        "async fn check_ancestry(",
    );

    assert!(write.contains("TransactionBehavior::Immediate"));
    assert!(write.contains("write_checked"));
    assert!(write.contains("tx.commit"));
    assert!(write.contains("tx.rollback"));
    assert!(checked.contains("check_cutover"));
    assert!(checked.contains("check_ancestry"));
    assert!(checked.contains("MemberPageTarget::Published"));
    let checked = compact(checked);
    assert!(checked.contains("projection.check"));
    assert!(checked.contains("projection.project"));
}

#[test]
fn hosted_conflict_copy_and_rename_paths_keep_visibility_consistent() {
    assert!(
        hosted_dispatch_uses_writer("sync_page", "apply_hosted_version"),
        "hosted rename and conflict copy must take the Space-scoped writer path"
    );
    let db_source = source("crates/wenlan-core/src/db/member_sync.rs");
    let checked = between(
        &db_source,
        "async fn write_checked(",
        "async fn check_ancestry(",
    );
    let import_source = source("crates/wenlan-core/src/shared_space/member_import.rs");
    let apply_version = between(
        &import_source,
        "pub(super) async fn apply_hosted_version(",
        "pub(super) async fn apply_hosted_create(",
    );

    assert!(apply_version.contains("MemberSyncConflict::Copy"));
    assert!(apply_version.contains("MemberSyncConflict::Merge"));
    assert!(
        checked.contains("available_title"),
        "hosted conflict copies and renames must use visible-title collision handling"
    );
    assert!(
        checked.contains("refresh_links"),
        "parent and conflict copy link visibility must refresh inside the checked transaction"
    );
}

#[test]
fn member_page_citation_changes_are_rejected_by_checked_edit_path() {
    assert!(
        hosted_dispatch_uses_writer("sync_page", "apply_hosted_version"),
        "a hosted edit must go through the citation-preserving writer"
    );
    let db_source = source("crates/wenlan-core/src/db/member_sync.rs");
    let checked = between(
        &db_source,
        "async fn write_checked(",
        "async fn check_ancestry(",
    );
    let checked_body = between(&db_source, "fn checked_body(", "async fn apply_page(");
    assert!(checked_body.contains("restore_unchanged_member_sentence_markers"));
    assert!(checked_body.contains("CitationsWouldChange"));
    let apply_page = between(
        &db_source,
        "async fn apply_page(",
        "async fn append_history(",
    );
    assert!(apply_page.contains("checked_body") && apply_page.contains("CitationsWouldChange"));
    assert!(
        checked.contains("apply_page"),
        "member edits must enter citation validation through checked write"
    );
}

#[test]
fn durable_host_deletion_blocks_member_recreate() {
    assert!(
        hosted_dispatch_uses_writer("sync_missing", "apply_hosted_create"),
        "a hosted create must check durable deletion before the first insert"
    );
    let db_source = source("crates/wenlan-core/src/db/member_sync.rs");
    let checked = between(
        &db_source,
        "async fn write_checked(",
        "async fn check_ancestry(",
    );
    assert!(
        checked.contains("shared_page_was_deleted_on"),
        "durable deletion marker must reject a member recreate/edit"
    );
}

#[test]
fn member_review_decision_and_staging_require_current_stay_and_lock() {
    let review_source = source("crates/wenlan-core/src/shared_space/review.rs");
    assert!(
        review_source.contains("MemberSyncOperation::Decide"),
        "hosted decisions must close cards through the current-stay transaction"
    );
    let db_source = source("crates/wenlan-core/src/db/member_sync.rs");
    let checked = between(
        &db_source,
        "async fn write_checked(",
        "async fn check_ancestry(",
    );
    let review_fn = between(
        &review_source,
        "pub(super) async fn review(",
        "\n    async fn accepted",
    );

    assert!(checked.contains("current_stay"));
    assert!(checked.contains("MemberSyncOperation::Stage"));
    assert!(
        checked.contains("page.review_required"),
        "stage requires an actively locked page"
    );
    assert!(
        checked.contains("card_stay.as_deref() != stay.as_deref()"),
        "decision must be bound to the current stay"
    );
    assert!(review_fn.contains("self.settings.hosted"));
    assert!(review_fn.contains("MemberSyncOperation::Decide"));
    assert!(
        review_fn.contains("!self.settings.hosted"),
        "legacy local card close is gated away from hosted review"
    );
    let review_file = parsed("crates/wenlan-core/src/shared_space/review.rs");
    let review_impl = impl_for(&review_file, "Pass");
    let stage_calls = method_calls(review_impl, "stage");
    assert!(has_call(&stage_calls, "write", "member_writer()"));
    assert!(
        between(&review_source, "async fn stage(", "async fn decide(")
            .contains("MemberSyncOperation::Stage")
    );
}

#[test]
fn member_lock_update_is_gated_by_current_scope_inside_the_transaction() {
    assert!(
        hosted_dispatch_uses_writer("sync_page", "apply_hosted_version"),
        "hosted review-lock changes must join the transactional Space-stay check"
    );
    let source = source("crates/wenlan-core/src/db/member_sync.rs");
    let checked = between(
        &source,
        "async fn write_checked(",
        "async fn check_ancestry(",
    );
    let access = checked.find("MemoryDB::member_page_access_on").unwrap();
    let apply = checked.find("apply_page(").unwrap();
    assert!(access < apply);
    assert!(checked.contains("page.review_required != expected.review_required"));
}

#[test]
fn deletion_manifest_requires_the_event_bound_archive() {
    let source_text = source("crates/wenlan-core/src/rulesync_team.rs");
    let manifest = between(
        &source_text,
        "pub fn hosted_file_list(",
        "pub fn read_hosted_rule_file(",
    );
    assert!(
        manifest.contains("deletion.archive_hash"),
        "an explicit deletion must survive when its durably bound archive is absent"
    );
    let download = source("crates/wenlan-core/src/team.rs");
    assert!(download.contains("deletion.archive_hash.as_ref() == Some(&read.sha)"));
}

#[test]
fn hosted_review_does_not_close_local_cards() {
    let review_source = source("crates/wenlan-core/src/shared_space/review.rs");
    assert!(review_source
        .contains("if let Some(id) = card.id.as_ref().filter(|_| !self.settings.hosted)"));
    assert!(review_source.contains(
        "if !self.settings.hosted {\n                    self.close_card(&card_id).await?;"
    ));
}

#[test]
fn focused_ast_inventory_is_stable_and_typed() {
    let import = parsed("crates/wenlan-core/src/shared_space/member_import.rs");
    let version = call_set(&all_calls(&import));
    assert!(version.contains(&Call {
        method: "write".into(),
        receiver: "member_writer()".into()
    }));

    let db = parsed("crates/wenlan-core/src/db/member_sync.rs");
    let writer = impl_for(&db, "MemberSyncWriter");
    let checked = method_calls(writer, "write_checked");
    assert!(has_call(&checked, "check_ancestry", "self"));

    // Pin the key typed check surfaces by name and signature shape. A rename,
    // removal, or untyped detour requires an explicit guard update.
    let source = source("crates/wenlan-core/src/db/member_sync.rs");
    for required in [
        "async fn check_ancestry(",
        "async fn check_cutover(conn: &Connection, projects: bool) -> Result<(), CheckError>",
        "MemoryDB::member_page_access_on(conn, id, &scope, MemberPageTarget::Published)",
        "MemoryDB::shared_page_was_deleted_on(conn, id)",
    ] {
        assert!(
            compact(&source).contains(&compact(required)),
            "missing typed safety check: {required}"
        );
    }
}
