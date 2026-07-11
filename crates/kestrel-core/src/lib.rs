pub mod agent;
pub mod cache;
pub mod config;
pub mod context;
pub mod environment;
pub mod graph;
pub mod inspect;
pub mod project;
pub mod providers;
pub mod settings;
pub mod symbols;
pub mod syntax;
pub mod verify;
pub mod workspace;

pub use agent::{
    agent_loop_system_prompt, agent_session_path, agent_system_prompt, apply_file_edits,
    builtin_tools, describe_call, execute_tool, git_commit_all, git_init, git_revert_all,
    git_review, load_agent_session, parse_file_edits, run_agent, save_agent_session, AgentEvent,
    AgentOutcome, AgentSession, AppliedEdit, FileEdit, GitReview,
};
pub use config::{load_config, Config, ConfigLoad};
pub use context::{
    assemble_context_prompt, build_context_pack, build_context_pack_for_query, estimate_tokens,
    ContextEntry, ContextPack,
};
pub use environment::{discover_environment, EnvironmentReport, ToolInfo, WslInfo};
pub use graph::{
    build_graph_from_files, build_project_graph, DependencyEdge, FileNode, ProjectGraph,
};
pub use inspect::{
    inspect_project, project_symbols, CommandKind, CommandSuggestion, FileInventory,
    LanguageSummary, ProjectInspection, ProjectMarker, SymbolSummary,
};
pub use project::{create_project, push_recent, NewProject, MAX_RECENTS};
pub use providers::{
    chat, chat_stream, run_turn, AgentMessage, ChatMessage, ChatRequest, ProviderConfig,
    ProviderKind, ToolCall, ToolResult, ToolSpec, TurnResult,
};
pub use settings::{
    load_settings, model_suggestions, model_suggestions_for, provider_preset, save_settings,
    ProviderSettings, Settings, UserInfo, PROVIDER_PRESETS,
};
pub use symbols::{
    extractor_for_language, extractor_for_path, symbols_for_file, FileSymbols, Import, Symbol,
    SymbolExtractor, SymbolKind,
};
pub use syntax::{highlight, language_from_extension, Language, Span, TokenKind};
pub use verify::{plan_verification, run_verification, StepResult, VerificationReport, VerifyStep};
pub use workspace::{
    create_dir, create_file, delete_entry, read_dir_entries, read_text_file, rename_entry,
    validate_entry_name, write_text_file, WorkspaceEntry,
};
