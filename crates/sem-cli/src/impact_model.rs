use sem_core::parser::graph::EntityInfo;

#[derive(Debug)]
pub enum ImpactQueryError {
    CacheReadFailed,
    MissingEntityQuery,
    EntityIdNotFound(String),
    EntityNotFound(String),
    EntityNotFoundInFile {
        name: String,
        file: String,
    },
    AmbiguousEntity {
        name: String,
        matches: Vec<EntityInfo>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImpactSource {
    Sidecar,
    Cloud,
    DiskCache,
    Local,
}

impl ImpactSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sidecar => "sidecar",
            Self::Cloud => "cloud",
            Self::DiskCache => "disk-cache",
            Self::Local => "local",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TestEvidence {
    #[default]
    CallGraph,
    LexicalFallback,
}

/// Backend-neutral result of an impact query.
pub struct ImpactReport {
    pub entity: EntityInfo,
    pub dependencies: Vec<EntityInfo>,
    pub dependents: Vec<EntityInfo>,
    pub impact: Vec<(EntityInfo, usize)>,
    pub tests: Vec<EntityInfo>,
    pub tests_truncated: bool,
    pub test_evidence: TestEvidence,
}

impl ImpactReport {
    pub fn for_entity(entity: EntityInfo) -> Self {
        Self {
            entity,
            dependencies: Vec::new(),
            dependents: Vec::new(),
            impact: Vec::new(),
            tests: Vec::new(),
            tests_truncated: false,
            test_evidence: TestEvidence::CallGraph,
        }
    }
}

/// A completed impact query together with the backend that answered it.
///
/// Keeping provenance attached to the report prevents the caller from
/// rendering a result with the wrong timing source.
pub struct ResolvedImpact {
    pub report: ImpactReport,
    pub source: ImpactSource,
}

impl ResolvedImpact {
    pub fn new(report: ImpactReport, source: ImpactSource) -> Self {
        Self { report, source }
    }
}
