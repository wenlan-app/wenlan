#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Method {
    Get,
    Post,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectorPrecedence {
    NotApplicable,
    Missing,
    BodyThenHeader,
    QueryThenHeader,
    HeaderOnly,
    PathOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    UnauthenticatedLocal,
    CallerAssertedAgentTrust,
    AuthenticatedPerson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeBinding {
    Global,
    MemorySpace,
    PageWorkspace,
    EntitySpace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionGate {
    NotApplicable,
    SingleIdMissing,
    SingleId404,
    BatchMissing,
    BatchFiltered,
    ParentCollectionMissing,
    ParentCollectionFiltered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownScopePolicy {
    NotApplicable,
    FallsBackUnscoped,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossScopePolicy {
    Forbidden,
    AggregateOnly,
    MixedRowsAndAggregates,
    GlobalRead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensitiveReadRoute {
    pub method: Method,
    pub path: &'static str,
    pub data_class: &'static str,
    pub selector_precedence: SelectorPrecedence,
    pub capability: Capability,
    pub scope_binding: ScopeBinding,
    pub selection_gate: SelectionGate,
    pub unknown_scope: UnknownScopePolicy,
    pub cross_scope_policy: CrossScopePolicy,
}

impl SensitiveReadRoute {
    pub const fn scope_contract_violation(self) -> bool {
        !matches!(self.scope_binding, ScopeBinding::Global)
            && (matches!(
                self.selection_gate,
                SelectionGate::SingleIdMissing
                    | SelectionGate::BatchMissing
                    | SelectionGate::ParentCollectionMissing
            ) || matches!(
                self.selector_precedence,
                SelectorPrecedence::Missing | SelectorPrecedence::NotApplicable
            ) || !matches!(self.unknown_scope, UnknownScopePolicy::Rejected))
    }
}

macro_rules! row {
    ($method:expr,$path:expr,$data:expr,$selector:expr,$capability:expr,$scope:expr,$gate:expr,$unknown:ident,$cross:expr) => {
        SensitiveReadRoute {
            method: $method,
            path: $path,
            data_class: $data,
            selector_precedence: $selector,
            capability: $capability,
            scope_binding: $scope,
            selection_gate: $gate,
            unknown_scope: $crate::lint::serving::routes::UnknownScopePolicy::$unknown,
            cross_scope_policy: $cross,
        }
    };
}

mod knowledge;
mod pages;
mod records;
mod retrieval;

pub fn sensitive_read_routes() -> impl Iterator<Item = &'static SensitiveReadRoute> {
    retrieval::ROUTES
        .iter()
        .chain(knowledge::ROUTES)
        .chain(pages::ROUTES)
        .chain(records::ROUTES)
}

pub fn route(method: Method, path: &str) -> Option<&'static SensitiveReadRoute> {
    sensitive_read_routes().find(|row| row.method == method && row.path == path)
}

pub fn scope_contract_violations() -> impl Iterator<Item = &'static SensitiveReadRoute> {
    sensitive_read_routes().filter(|row| row.scope_contract_violation())
}
