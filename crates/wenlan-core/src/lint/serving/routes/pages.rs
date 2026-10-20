use super::{
    Capability::*,
    CrossScopePolicy::*,
    Method::*,
    ScopeBinding::*,
    SelectionGate::{NotApplicable as NoGate, ParentCollectionFiltered, SingleId404},
    SelectorPrecedence::{BodyThenHeader, HeaderOnly, QueryThenHeader},
    SensitiveReadRoute,
};

#[rustfmt::skip]
pub(super) const ROUTES: &[SensitiveReadRoute] = &[
    row!(Get,"/api/pages/recent","page_list",HeaderOnly,UnauthenticatedLocal,PageWorkspace,NoGate,Rejected,Forbidden),
    row!(Get,"/api/pages/recent-changes","page_activity",HeaderOnly,UnauthenticatedLocal,PageWorkspace,NoGate,Rejected,Forbidden),
    row!(Get,"/api/pages","page_list",QueryThenHeader,UnauthenticatedLocal,PageWorkspace,NoGate,Rejected,Forbidden),
    row!(Post,"/api/pages/search","page_search",BodyThenHeader,UnauthenticatedLocal,PageWorkspace,NoGate,Rejected,Forbidden),
    row!(Post,"/api/pages/export","page_export",HeaderOnly,UnauthenticatedLocal,PageWorkspace,NoGate,Rejected,Forbidden),
    row!(Get,"/api/pages/orphan-links","page_links",HeaderOnly,UnauthenticatedLocal,PageWorkspace,NoGate,Rejected,Forbidden),
    row!(Get,"/api/pages/{id}","page_detail",HeaderOnly,UnauthenticatedLocal,PageWorkspace,SingleId404,Rejected,Forbidden),
    row!(Post,"/api/pages/{id}/move","page_storage_move",HeaderOnly,UnauthenticatedLocal,PageWorkspace,SingleId404,Rejected,Forbidden),
    row!(Post,"/api/pages/{id}/export","page_export",HeaderOnly,UnauthenticatedLocal,PageWorkspace,SingleId404,Rejected,Forbidden),
    row!(Post,"/api/pages/{id}/rename","page_title",HeaderOnly,UnauthenticatedLocal,PageWorkspace,SingleId404,Rejected,Forbidden),
    row!(Get,"/api/pages/{id}/sources","page_sources",HeaderOnly,UnauthenticatedLocal,PageWorkspace,ParentCollectionFiltered,Rejected,Forbidden),
    row!(Get,"/api/pages/{id}/links","page_links",HeaderOnly,UnauthenticatedLocal,PageWorkspace,ParentCollectionFiltered,Rejected,Forbidden),
    row!(Get,"/api/pages/{id}/revisions","page_revisions",HeaderOnly,UnauthenticatedLocal,PageWorkspace,ParentCollectionFiltered,Rejected,Forbidden),
    row!(Get,"/api/pages/basis-pending","page_basis",QueryThenHeader,UnauthenticatedLocal,PageWorkspace,NoGate,Rejected,Forbidden),
    row!(Get,"/api/pages/{id}/basis/{basis_id}/diff","page_basis",HeaderOnly,UnauthenticatedLocal,PageWorkspace,SingleId404,Rejected,Forbidden),
    row!(Post,"/api/pages/code-basis/recheck","page_code_basis",QueryThenHeader,UnauthenticatedLocal,PageWorkspace,NoGate,Rejected,Forbidden),
    row!(Get,"/api/pages/{id}/map","page_map",HeaderOnly,UnauthenticatedLocal,PageWorkspace,ParentCollectionFiltered,Rejected,Forbidden),
    row!(Get,"/api/source-annotations","source_annotation",QueryThenHeader,UnauthenticatedLocal,PageWorkspace,SingleId404,Rejected,Forbidden),
    row!(Post,"/api/source-annotations","source_annotation",BodyThenHeader,UnauthenticatedLocal,PageWorkspace,SingleId404,Rejected,Forbidden),
];
