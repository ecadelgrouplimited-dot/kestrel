pub mod cache;
pub mod context;
pub mod graph;
pub mod inspect;
pub mod symbols;
pub mod verify;

pub use context::{
    build_context_pack, build_context_pack_for_query, estimate_tokens, ContextEntry, ContextPack,
};
pub use graph::{
    build_graph_from_files, build_project_graph, DependencyEdge, FileNode, ProjectGraph,
};
pub use inspect::{
    inspect_project, project_symbols, CommandKind, CommandSuggestion, FileInventory,
    LanguageSummary, ProjectInspection, ProjectMarker, SymbolSummary,
};
pub use symbols::{
    extractor_for_language, extractor_for_path, symbols_for_file, FileSymbols, Import, Symbol,
    SymbolExtractor, SymbolKind,
};
pub use verify::{plan_verification, run_verification, StepResult, VerificationReport, VerifyStep};
