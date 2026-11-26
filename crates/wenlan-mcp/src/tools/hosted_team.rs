// SPDX-License-Identifier: AGPL-3.0-only
//! Typed tool boundary for an authenticated hosted team member.

use super::*;
use rmcp::model::Tool;
use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::Arc;
use wenlan_types::responses::MemberRenamePageResponse;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemberGrant {
    Query,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemberTool {
    Brief,
    Recall,
    PageSources,
    SpaceRules,
    RuleUseReport,
    ReadPage,
    ListPages,
    WritePage,
    RenamePage,
    ArchivePage,
}

impl MemberTool {
    const fn name(self) -> &'static str {
        match self {
            Self::Brief => "brief",
            Self::Recall => "recall",
            Self::PageSources => "get_page_sources",
            Self::SpaceRules => "read_space_rules",
            Self::RuleUseReport => "report_space_rule_use",
            Self::ReadPage => "read_page",
            Self::ListPages => "list_pages",
            Self::WritePage => "write_page",
            Self::RenamePage => "rename_page",
            Self::ArchivePage => "delete_page",
        }
    }

    const fn grant(self) -> MemberGrant {
        match self {
            Self::Brief | Self::Recall | Self::PageSources | Self::SpaceRules => MemberGrant::Query,
            Self::RuleUseReport
            | Self::ReadPage
            | Self::ListPages
            | Self::WritePage
            | Self::RenamePage
            | Self::ArchivePage => MemberGrant::Write,
        }
    }
}

const MEMBER_TOOLS: &[MemberTool] = &[
    MemberTool::Brief,
    MemberTool::Recall,
    MemberTool::PageSources,
    MemberTool::SpaceRules,
    MemberTool::RuleUseReport,
    MemberTool::ReadPage,
    MemberTool::ListPages,
    MemberTool::WritePage,
    MemberTool::RenamePage,
    MemberTool::ArchivePage,
];

fn grant(profile: ToolProfile) -> Option<MemberGrant> {
    match profile {
        ToolProfile::QueryOnly => Some(MemberGrant::Query),
        ToolProfile::WebWrite => Some(MemberGrant::Write),
        ToolProfile::Standard | ToolProfile::Daily | ToolProfile::Plugin => None,
    }
}

pub(super) fn allowed(name: &str, profile: ToolProfile) -> bool {
    grant(profile).is_some_and(|grant| {
        MEMBER_TOOLS.iter().any(|tool| {
            tool.name() == name && (grant == MemberGrant::Write || tool.grant() == grant)
        })
    })
}

pub(super) fn rules_allowed(name: &str, profile: ToolProfile) -> bool {
    matches!(name, "read_space_rules" | "report_space_rule_use") && allowed(name, profile)
}

fn registered_tool(name: &str, profile: ToolProfile) -> Option<MemberTool> {
    let grant = grant(profile)?;
    MEMBER_TOOLS
        .iter()
        .copied()
        .find(|tool| tool.name() == name && (grant == MemberGrant::Write || tool.grant() == grant))
}

pub(super) fn apply_output_schema(tool: &mut Tool) {
    tool.output_schema = match tool.name.as_ref() {
        "brief" => Some(schema_for::<MemberBrief>()),
        "recall" => Some(schema_for::<MemberRecall>()),
        "get_page_sources" => Some(schema_for::<MemberPageSources>()),
        "read_space_rules" => Some(schema_for::<MemberRules>()),
        _ => tool.output_schema.take(),
    };
}

fn schema_for<T: JsonSchema>() -> Arc<serde_json::Map<String, serde_json::Value>> {
    let value = serde_json::to_value(schemars::schema_for!(T))
        .expect("hosted member output schemas must serialize");
    Arc::new(
        value
            .as_object()
            .expect("hosted member output schemas are objects")
            .clone(),
    )
}

pub(super) async fn dispatch(
    server: &WenlanMcpServer,
    name: &str,
    arguments: Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<CallToolResult, McpError> {
    if server.tool_profile == ToolProfile::QueryOnly && !allowed(name, server.tool_profile) {
        return Ok(member_error_owned(format!(
            "{QUERY_ONLY_REFUSAL_MESSAGE} {}",
            read_only_team_note!()
        )));
    }
    if matches!(name, "capture" | "forget") {
        return Ok(member_error(
            "Memories are saved and managed in your own Wenlan.",
        ));
    }
    let Some(tool) = registered_tool(name, server.tool_profile) else {
        return Ok(member_error(
            "This tool is unavailable in this shared Space.",
        ));
    };
    let args = serde_json::Value::Object(arguments.unwrap_or_default());
    let result = match tool {
        MemberTool::Brief => {
            let params = parse::<BriefParams>(args)?;
            member_brief(server, params).await
        }
        MemberTool::Recall => {
            let params = parse::<RecallParams>(args)?;
            member_recall(server, params).await
        }
        MemberTool::PageSources => {
            let params = parse::<GetPageSourcesParams>(args)?;
            member_sources(server, params).await
        }
        MemberTool::SpaceRules => {
            let params = parse::<ReadSpaceRulesParams>(args)?;
            member_rules(server, params).await
        }
        MemberTool::RuleUseReport => {
            let params = parse::<ReportSpaceRuleUseParams>(args)?;
            member_rule_report(server, params).await
        }
        MemberTool::ReadPage => {
            let params = parse::<ReadPageParams>(args)?;
            member_read_page(server, params).await
        }
        MemberTool::ListPages => {
            let params = parse::<ListPagesParams>(args)?;
            member_list_pages(server, params).await
        }
        MemberTool::WritePage => {
            let params = parse::<WritePageParams>(args)?;
            member_write_page(server, params).await
        }
        MemberTool::RenamePage => {
            let params = parse::<RenamePageParams>(args)?;
            member_rename_page(server, params).await
        }
        MemberTool::ArchivePage => {
            let params = parse::<DeletePageParams>(args)?;
            member_archive_page(server, params).await
        }
    }?;
    result.into_call_tool_result()
}

fn parse<T: DeserializeOwned>(value: serde_json::Value) -> Result<T, McpError> {
    serde_json::from_value(value).map_err(|error| McpError::invalid_params(error.to_string(), None))
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct MemberSearchHit {
    id: String,
    source_id: String,
    title: String,
    content: String,
    source: String,
    space: Option<String>,
    version: i64,
    last_modified: i64,
    raw_score: f32,
    confirmed: Option<bool>,
    chunk_index: i32,
    score: f32,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
enum MemberBriefState {
    SpaceNotResolved,
    BriefNotCreated,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemberBriefContext {
    query: String,
    results: Vec<MemberSearchHit>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemberBrief {
    state: MemberBriefState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    space: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    related_context: Option<MemberBriefContext>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemberRecall {
    results: [(); 0],
    took_ms: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    supplemental_pages: Vec<MemberSearchHit>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemberPageSources {
    page_id: String,
    sources: [(); 0],
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemberRules {
    space_id: String,
    space_name: String,
    lines: Vec<MemberRule>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum MemberRuleKind {
    Rule,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemberRule {
    id: String,
    kind: MemberRuleKind,
    text: String,
    source_page_id: String,
    source_page_title: String,
    source_revision: i64,
    source_revision_at: i64,
    source_quote: String,
    person_edited: bool,
    general: bool,
    hand_written: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberRuleReport {
    recorded: bool,
    #[serde(default)]
    rule_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberCreateReceipt {
    id: String,
    attached_to: Option<String>,
    space: Option<String>,
    space_source: Option<wenlan_types::WriteSpaceSource>,
    write_outcome: Option<wenlan_types::WriteOutcome>,
    #[serde(default)]
    basis: Vec<MemberBasisReceipt>,
    next_step: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberWriteReceipt {
    ok: bool,
    write_outcome: Option<wenlan_types::responses::PageWriteOutcome>,
    revision_card_id: Option<String>,
    gated: bool,
    #[serde(default)]
    basis: Vec<MemberBasisReceipt>,
    #[serde(default)]
    sentence_proposals: Vec<String>,
    next_step: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberBasisReceipt {
    page_id: String,
    title: String,
    state: String,
    from_version: i64,
    to_version: Option<i64>,
    minor_only: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberRemovalReceipt {
    status: wenlan_types::responses::PageRemovalStatus,
    restorable: bool,
    #[allow(dead_code)]
    version: Option<i64>,
    #[allow(dead_code)]
    projection_status: Option<wenlan_types::responses::PageProjectionStatus>,
}

enum MemberWriteResult {
    Created(MemberCreateReceipt),
    Updated {
        page_id: String,
        receipt: MemberWriteReceipt,
    },
}

enum MemberOutput {
    Brief(MemberBrief),
    Recall(MemberRecall),
    PageSources(MemberPageSources),
    SpaceRules(MemberRules),
    RuleUseReport(MemberRuleReport),
    ReadPage(Box<wenlan_types::member_view::HostedPageRead>),
    ListPages(wenlan_types::member_view::HostedPageList),
    Write(MemberWriteResult),
    WriteConflict {
        page_id: String,
        expected_version: i64,
        current_version: Option<i64>,
    },
    Rename(MemberRenamePageResponse),
    Archive(MemberRemovalReceipt),
    Error(&'static str),
}

impl MemberOutput {
    fn into_call_tool_result(self) -> Result<CallToolResult, McpError> {
        match self {
            Self::Brief(output) => structured_result(output),
            Self::Recall(output) => structured_result(output),
            Self::PageSources(output) => structured_result(output),
            Self::SpaceRules(output) => structured_result(output),
            Self::RuleUseReport(output) => structured_result(output),
            Self::ReadPage(output) => structured_result(output),
            Self::ListPages(output) => structured_result(output),
            Self::Write(MemberWriteResult::Created(receipt)) => {
                Ok(member_success(render_create(&receipt)))
            }
            Self::Write(MemberWriteResult::Updated { page_id, receipt }) => {
                Ok(member_success(render_write(&page_id, &receipt)))
            }
            Self::WriteConflict {
                page_id,
                expected_version,
                current_version,
            } => {
                let current = current_version
                    .map(|version| format!(" It is now version {version}."))
                    .unwrap_or_default();
                Ok(member_error_owned(format!(
                    "Page {page_id} changed after version {expected_version}.{current} Read it again and retry."
                )))
            }
            Self::Rename(receipt) => Ok(member_success(render_rename(&receipt))),
            Self::Archive(receipt) => Ok(member_success(render_archive(&receipt))),
            Self::Error(message) => Ok(member_error(message)),
        }
    }
}

pub(super) fn structured_result<T: Serialize>(output: T) -> Result<CallToolResult, McpError> {
    query_success(output)
}

fn member_success(text: String) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(text)])
}

fn member_error(text: &'static str) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(text)])
}

fn member_error_owned(text: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(text)])
}

fn render_create(receipt: &MemberCreateReceipt) -> String {
    let mut text = match receipt.attached_to.as_deref() {
        Some(page_id) => format!(
            "Attached sources to existing page {page_id}; the new prose was not saved. Read that page before refreshing it."
        ),
        None if matches!(
            receipt.write_outcome,
            Some(wenlan_types::WriteOutcome::ResolvedExisting)
        ) =>
        {
            format!(
                "Page {} already exists; no duplicate was created. Read it before updating it.",
                receipt.id
            )
        }
        None if matches!(
            receipt.write_outcome,
            Some(wenlan_types::WriteOutcome::Unknown)
        ) =>
        {
            format!("Page request completed for {}.", receipt.id)
        }
        None => format!("Created page {}.", receipt.id),
    };
    append_write_receipt(
        &mut text,
        receipt.space.as_deref(),
        receipt.space_source,
        receipt.write_outcome,
    );
    append_member_bases(&mut text, &receipt.basis);
    if !receipt.next_step.trim().is_empty() {
        text.push_str(&format!(" {}", receipt.next_step));
    }
    text
}

fn render_write(page_id: &str, receipt: &MemberWriteReceipt) -> String {
    let mut text = if receipt.gated {
        let card = receipt
            .revision_card_id
            .as_deref()
            .map(|id| format!(" Revision card {id} is waiting for review."))
            .unwrap_or_default();
        format!(
            "Page {page_id} was not changed.{card} {}",
            receipt.next_step
        )
    } else {
        match receipt.write_outcome {
            Some(wenlan_types::responses::PageWriteOutcome::Wrote) => {
                format!("Updated page {page_id}. {}", receipt.next_step)
            }
            Some(wenlan_types::responses::PageWriteOutcome::Unchanged) => {
                format!(
                    "Page {page_id} was already up to date. {}",
                    receipt.next_step
                )
            }
            Some(
                wenlan_types::responses::PageWriteOutcome::Refused
                | wenlan_types::responses::PageWriteOutcome::Contended,
            ) => format!("Page {page_id} was not updated. {}", receipt.next_step),
            Some(wenlan_types::responses::PageWriteOutcome::Gated) => {
                format!(
                    "Page {page_id} is waiting for review. {}",
                    receipt.next_step
                )
            }
            Some(wenlan_types::responses::PageWriteOutcome::Unknown) | None if receipt.ok => {
                format!("Updated page {page_id}. {}", receipt.next_step)
            }
            Some(wenlan_types::responses::PageWriteOutcome::Unknown) | None => {
                format!("Page {page_id} was not updated. {}", receipt.next_step)
            }
        }
    };
    append_member_bases(&mut text, &receipt.basis);
    if !receipt.sentence_proposals.is_empty() {
        text.push_str(&format!(
            " {} sentence proposal(s) are waiting for review.",
            receipt.sentence_proposals.len()
        ));
    }
    text
}

fn append_member_bases(text: &mut String, bases: &[MemberBasisReceipt]) {
    if bases.is_empty() {
        return;
    }
    let details = bases
        .iter()
        .map(|basis| {
            let span = basis.to_version.map_or_else(
                || format!("from v{}", basis.from_version),
                |to| format!("v{} to v{to}", basis.from_version),
            );
            let minor = if basis.minor_only {
                ", minor edits only"
            } else {
                ""
            };
            format!(
                "{} ({}; {span}{minor}; page {})",
                basis.title, basis.state, basis.page_id
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    text.push_str(&format!(" Bases: {details}."));
}

fn render_rename(receipt: &MemberRenamePageResponse) -> String {
    let mut text = format!(
        "Renamed page {} to \"{}\" (version {}).",
        receipt.id, receipt.title, receipt.version
    );
    for (count, what) in [
        (receipt.links_updated, "had links updated to the new title"),
        (
            receipt.links_pending_review,
            "have link updates waiting for review",
        ),
        (receipt.links_not_updated, "still link to the old title"),
    ] {
        if count > 0 {
            text.push_str(&format!(" Other pages that {what}: {count}."));
        }
    }
    text
}

fn render_archive(receipt: &MemberRemovalReceipt) -> String {
    match (receipt.status, receipt.restorable) {
        (wenlan_types::responses::PageRemovalStatus::Archived, true) => {
            "Page archived. Its history is kept, and the person can restore it from the archive."
                .to_owned()
        }
        (wenlan_types::responses::PageRemovalStatus::Archived, false) => {
            "Page is not active; nothing was archived.".to_owned()
        }
        (wenlan_types::responses::PageRemovalStatus::Deleted, _) => "Page deleted.".to_owned(),
    }
}

/// A connection without the write profile is read-only (a link-only member's);
/// only that one is told how to get edit rights. A member who can edit and
/// still gets a 403 is not.
fn closed_error(error: &WenlanError, server: &WenlanMcpServer) -> &'static str {
    match error {
        WenlanError::Api { status: 404, .. } => "This item was not found in the shared Space.",
        WenlanError::Api { status: 409, .. } => "The page changed. Read it again and retry.",
        WenlanError::Api { status: 422, .. } => "The request was refused; nothing was changed.",
        WenlanError::Api { status: 403, .. } if server.tool_profile != ToolProfile::WebWrite => {
            concat!(
                "This tool is unavailable in this shared Space. ",
                read_only_team_note!()
            )
        }
        WenlanError::Api { status: 403, .. } => "This tool is unavailable in this shared Space.",
        WenlanError::Api { .. } => "The shared Space request could not be completed.",
        WenlanError::Unreachable(_) | WenlanError::Busy | WenlanError::NoAnswer => {
            "The shared Space service could not complete this request."
        }
        WenlanError::Deserialize | WenlanError::ResponseTooLarge => {
            "The shared Space returned an unsupported response."
        }
        WenlanError::SourceScopeNotConfirmed => {
            "The requested source scope was not confirmed; no source text was returned."
        }
    }
}

async fn member_brief(
    server: &WenlanMcpServer,
    params: BriefParams,
) -> Result<MemberOutput, McpError> {
    let client = server.session_client(params.session.as_deref()).await;
    let request = wenlan_types::BriefReadRequest {
        topic: params.topic,
        space: effective_space(&params.space),
        // Members get lines only; item details stay with the host.
        item: None,
        legacy_context_limit: None,
    };
    let response: MemberBrief = match client.post("/api/brief", &request).await {
        Ok(response) => response,
        Err(error) => return Ok(MemberOutput::Error(closed_error(&error, server))),
    };
    Ok(MemberOutput::Brief(response))
}

async fn member_recall(
    server: &WenlanMcpServer,
    params: RecallParams,
) -> Result<MemberOutput, McpError> {
    let client = server.session_client(params.session.as_deref()).await;
    let request = recall_search_request(params);
    let response: MemberRecall = match client.post("/api/memory/search", &request).await {
        Ok(response) => response,
        Err(error) => return Ok(MemberOutput::Error(closed_error(&error, server))),
    };
    Ok(MemberOutput::Recall(response))
}

async fn member_sources(
    server: &WenlanMcpServer,
    params: GetPageSourcesParams,
) -> Result<MemberOutput, McpError> {
    if !is_opaque_page_id(&params.page_id) {
        return Err(McpError::invalid_params(
            "Invalid page id. Use the page's opaque identifier, not a URL or file path.",
            None,
        ));
    }
    let _: [(); 0] = match server
        .client
        .get(&format!("/api/pages/{}/sources", params.page_id))
        .await
    {
        Ok(response) => response,
        Err(error) => return Ok(MemberOutput::Error(closed_error(&error, server))),
    };
    Ok(MemberOutput::PageSources(MemberPageSources {
        page_id: params.page_id,
        sources: [],
    }))
}

async fn member_rules(
    server: &WenlanMcpServer,
    params: ReadSpaceRulesParams,
) -> Result<MemberOutput, McpError> {
    let result: Result<MemberRules, WenlanError> = match effective_space(&params.space) {
        Some(space) => {
            server
                .client
                .get_with_query("/api/skills/rules", &[("space", space)])
                .await
        }
        None => server.client.get("/api/skills/rules").await,
    };
    let response = match result {
        Ok(response) => response,
        Err(error) => return Ok(MemberOutput::Error(closed_error(&error, server))),
    };
    Ok(MemberOutput::SpaceRules(response))
}

async fn member_rule_report(
    server: &WenlanMcpServer,
    params: ReportSpaceRuleUseParams,
) -> Result<MemberOutput, McpError> {
    if params.rule_id.trim().is_empty() {
        return Err(McpError::invalid_params("rule_id is required", None));
    }
    let request = wenlan_types::skills::SkillReportRequest {
        space: effective_space(&params.space).unwrap_or_default(),
        rule_id: params.rule_id,
        report: match params.outcome {
            SpaceRuleUseOutcome::ChangedPlan => wenlan_types::skills::SkillReportKind::ChangedPlan,
        },
    };
    let response: MemberRuleReport = match server.client.post("/api/skills/record", &request).await
    {
        Ok(response) => response,
        Err(error) => return Ok(MemberOutput::Error(closed_error(&error, server))),
    };
    Ok(MemberOutput::RuleUseReport(response))
}

async fn member_read_page(
    server: &WenlanMcpServer,
    params: ReadPageParams,
) -> Result<MemberOutput, McpError> {
    if !is_opaque_page_id(&params.page_id) {
        return Err(McpError::invalid_params(
            "Invalid page id. Use the page's opaque identifier, not a URL or file path.",
            None,
        ));
    }
    let client = server.session_client(params.session.as_deref()).await;
    let response: wenlan_types::member_view::MemberPageRead =
        match client.get(&format!("/api/pages/{}", params.page_id)).await {
            Ok(response) => response,
            Err(error) => return Ok(MemberOutput::Error(closed_error(&error, server))),
        };
    Ok(MemberOutput::ReadPage(Box::new(response.into())))
}

async fn member_list_pages(
    server: &WenlanMcpServer,
    params: ListPagesParams,
) -> Result<MemberOutput, McpError> {
    let limit = params.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(McpError::invalid_params(
            "limit must be between 1 and 200",
            None,
        ));
    }
    let space = crate::lock_state::locked_space().or_else(|| {
        params
            .space
            .as_deref()
            .map(str::trim)
            .filter(|space| !space.is_empty())
            .map(str::to_owned)
    });
    let mut query = vec![
        (
            "status",
            params.status.unwrap_or_else(|| "active".to_owned()),
        ),
        ("limit", limit.to_string()),
        ("offset", params.offset.unwrap_or(0).to_string()),
    ];
    if let Some(space) = space {
        query.push(("space", space));
    }
    let response: wenlan_types::member_view::MemberPages<
        wenlan_types::member_view::MemberPageEntry,
    > = match server.client.get_with_query("/api/pages", &query).await {
        Ok(response) => response,
        Err(error) => return Ok(MemberOutput::Error(closed_error(&error, server))),
    };
    Ok(MemberOutput::ListPages(response.into()))
}

async fn member_write_page(
    server: &WenlanMcpServer,
    params: WritePageParams,
) -> Result<MemberOutput, McpError> {
    if !params.code_basis.is_empty() {
        return Ok(MemberOutput::Error(
            "code_basis is not available over remote connections. Use local MCP on the machine running Wenlan.",
        ));
    }
    if params.person_asked.is_some() {
        return Ok(MemberOutput::Error(
            "person_asked is not available over remote connections. Nothing was written; the person can accept the change in the Wenlan app.",
        ));
    }
    let client = server.session_client(params.session.as_deref()).await;
    match params.page_id {
        Some(page_id) => {
            if !is_opaque_page_id(&page_id) {
                return Ok(MemberOutput::Error(WEB_WRITE_BAD_ID_MESSAGE));
            }
            let Some(expected_version) = params.expected_version else {
                return Ok(MemberOutput::Error(TEAM_REFRESH_NEEDS_VERSION));
            };
            if expected_version < 1 {
                return Err(McpError::invalid_params(
                    "expected_version must be at least 1",
                    None,
                ));
            }
            let request = wenlan_types::requests::RefreshPageRequest {
                organize_space: params
                    .organize_run_id
                    .as_ref()
                    .and_then(|_| effective_space(&params.space)),
                organize_run_id: params.organize_run_id,
                citation_targets: params
                    .citation_targets
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                content: params.content,
                source_memory_ids: params.source_memory_ids,
                summary: params.summary,
                expected_version: Some(expected_version),
                basis: params.basis.into_iter().map(Into::into).collect(),
                code_basis: Vec::new(),
                minor: params.minor,
                person_asked: None,
            };
            let path = format!("/api/pages/{page_id}");
            let receipt: MemberWriteReceipt = match client.put_save(&path, &request).await {
                Ok(receipt) => receipt,
                Err(WenlanError::Api { status: 409, .. }) => {
                    let current_version = client
                        .get::<wenlan_types::member_view::MemberPageRead>(&path)
                        .await
                        .ok()
                        .map(|page| page.page.page.version);
                    return Ok(MemberOutput::WriteConflict {
                        page_id,
                        expected_version,
                        current_version,
                    });
                }
                Err(error) => return Ok(MemberOutput::Error(closed_error(&error, server))),
            };
            Ok(MemberOutput::Write(MemberWriteResult::Updated {
                page_id,
                receipt,
            }))
        }
        None => {
            let Some(title) = params.title else {
                return Err(McpError::invalid_params(
                    "title is required when creating a page (no page_id given)",
                    None,
                ));
            };
            if title.trim().is_empty() {
                return Err(McpError::invalid_params("title is required", None));
            }
            let request = wenlan_types::requests::CreatePageWithBasisRequest {
                organize_run_id: params.organize_run_id,
                citation_targets: params
                    .citation_targets
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                create: wenlan_types::requests::CreateConceptRequest {
                    title,
                    content: params.content,
                    summary: params.summary,
                    entity_id: params.entity_id,
                    space: effective_space(&params.space).into(),
                    source_memory_ids: params.source_memory_ids,
                    creation_kind: None,
                    workspace: None,
                },
                basis: params.basis.into_iter().map(Into::into).collect(),
                code_basis: Vec::new(),
            };
            let receipt: MemberCreateReceipt = match client.post_save("/api/pages", &request).await
            {
                Ok(receipt) => receipt,
                Err(WenlanError::Api { status: 409, .. }) => {
                    return Ok(MemberOutput::Error(
                        "The page could not be created. Choose another title or continue with a page you already know.",
                    ));
                }
                Err(error) => return Ok(MemberOutput::Error(closed_error(&error, server))),
            };
            Ok(MemberOutput::Write(MemberWriteResult::Created(receipt)))
        }
    }
}

async fn member_rename_page(
    server: &WenlanMcpServer,
    params: RenamePageParams,
) -> Result<MemberOutput, McpError> {
    if !is_opaque_page_id(&params.page_id) {
        return Ok(MemberOutput::Error(WEB_WRITE_BAD_ID_MESSAGE));
    }
    if params.title.trim().is_empty() || params.expected_version < 1 {
        return Err(McpError::invalid_params(
            "title must be nonblank and expected_version must be at least 1",
            None,
        ));
    }
    let request = wenlan_types::requests::RenamePageRequest {
        title: params.title,
        expected_version: params.expected_version,
    };
    let path = format!("/api/pages/{}/rename", url_encode_simple(&params.page_id));
    let receipt: MemberRenamePageResponse = match server.client.post(&path, &request).await {
        Ok(receipt) => receipt,
        Err(error) => return Ok(MemberOutput::Error(closed_error(&error, server))),
    };
    Ok(MemberOutput::Rename(receipt))
}

async fn member_archive_page(
    server: &WenlanMcpServer,
    params: DeletePageParams,
) -> Result<MemberOutput, McpError> {
    if !is_opaque_page_id(&params.page_id) {
        return Ok(MemberOutput::Error(WEB_WRITE_BAD_ID_MESSAGE));
    }
    let client = server.session_client(None).await;
    let receipt: MemberRemovalReceipt = match client
        .delete(&format!(
            "/api/pages/{}",
            url_encode_simple(&params.page_id)
        ))
        .await
    {
        Ok(receipt) => receipt,
        Err(error) => return Ok(MemberOutput::Error(closed_error(&error, server))),
    };
    Ok(MemberOutput::Archive(receipt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_read_only_member_is_told_how_to_get_edit_rights_on_a_403() {
        let member = |profile| {
            WenlanMcpServer::new_with_identity(
                crate::client::WenlanClient::new("http://127.0.0.1:19999".into()),
                crate::tools::TransportMode::Http,
                crate::tools::AgentIdentity::new("remote-mcp".into(), false)
                    .for_team_member("m".into()),
                None,
            )
            .with_tool_profile(profile)
        };
        let forbidden = WenlanError::Api {
            status: 403,
            reason: None,
        };
        let read_only = closed_error(&forbidden, &member(ToolProfile::QueryOnly));
        assert!(
            read_only.contains("sign in with Google or GitHub"),
            "{read_only}"
        );
        let writer = closed_error(&forbidden, &member(ToolProfile::WebWrite));
        assert_eq!(writer, "This tool is unavailable in this shared Space.");
    }

    #[test]
    fn member_tool_catalog_matches_each_grant() {
        let query = ["brief", "recall", "get_page_sources", "read_space_rules"];
        let write = [
            "brief",
            "recall",
            "get_page_sources",
            "read_space_rules",
            "report_space_rule_use",
            "read_page",
            "list_pages",
            "write_page",
            "rename_page",
            "delete_page",
        ];
        let actual_query: Vec<_> = MEMBER_TOOLS
            .iter()
            .filter(|tool| tool.grant() == MemberGrant::Query)
            .map(|tool| tool.name())
            .collect();
        let actual_write: Vec<_> = MEMBER_TOOLS.iter().map(|tool| tool.name()).collect();
        assert_eq!(actual_query, query);
        assert_eq!(actual_write, write);
        assert!(query
            .iter()
            .all(|name| allowed(name, ToolProfile::QueryOnly)));
        assert!(write
            .iter()
            .all(|name| allowed(name, ToolProfile::WebWrite)));
        assert!(!allowed("capture", ToolProfile::WebWrite));
        assert!(!allowed("forget", ToolProfile::WebWrite));
        assert!(!allowed("read_page", ToolProfile::QueryOnly));
        assert!(!allowed("write_page", ToolProfile::Standard));
    }

    #[test]
    fn hosted_member_list_uses_the_same_exact_catalog() {
        let server_for = |profile| {
            WenlanMcpServer::new_with_identity(
                WenlanClient::new("http://127.0.0.1:19999".into()),
                TransportMode::Http,
                AgentIdentity::new("remote-mcp".into(), false)
                    .for_team_member("fixture-member".into()),
                None,
            )
            .with_tool_profile(profile)
        };
        let mut query: Vec<_> = server_for(ToolProfile::QueryOnly)
            .visible_tools()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        query.sort();
        assert_eq!(
            query,
            ["brief", "get_page_sources", "read_space_rules", "recall"]
        );

        let mut write: Vec<_> = server_for(ToolProfile::WebWrite)
            .visible_tools()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        write.sort();
        assert_eq!(
            write,
            [
                "brief",
                "delete_page",
                "get_page_sources",
                "list_pages",
                "read_page",
                "read_space_rules",
                "recall",
                "rename_page",
                "report_space_rule_use",
                "write_page",
            ]
        );
    }
}
