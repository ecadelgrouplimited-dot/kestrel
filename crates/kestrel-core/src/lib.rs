pub mod cache;
pub mod config;
pub mod context;
pub mod environment;
pub mod graph;
pub mod inspect;
pub mod providers;
pub mod settings;
pub mod symbols;
pub mod verify;

pub use config::{load_config, Config, ConfigLoad};
pub use context::{
    build_context_pack, build_context_pack_for_query, estimate_tokens, ContextEntry, ContextPack,
};
pub use environment::{discover_environment, EnvironmentReport, ToolInfo, WslInfo};
pub use graph::{
    build_graph_from_files, build_project_graph, DependencyEdge, FileNode, ProjectGraph,
};
pub use inspect::{
    inspect_project, project_symbols, CommandKind, CommandSuggestion, FileInventory,
    LanguageSummary, ProjectInspection, ProjectMarker, SymbolSummary,
};
pub use providers::{chat, ChatMessage, ChatRequest, ProviderConfig, ProviderKind};
pub use settings::{
    load_settings, provider_preset, save_settings, ProviderSettings, Settings, UserInfo,
    PROVIDER_PRESETS,
};
pub use symbols::{
    extractor_for_language, extractor_for_path, symbols_for_file, FileSymbols, Import, Symbol,
    SymbolExtractor, SymbolKind,
};
pub use verify::{plan_verification, run_verification, StepResult, VerificationReport, VerifyStep};
