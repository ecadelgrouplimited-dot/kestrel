pub mod agent;
pub mod brand;
pub mod browser;
pub mod cache;
pub mod codenav;
pub mod config;
pub mod context;
pub mod diagnostics;
pub mod docwrite;
pub mod environment;
pub mod format;
pub mod graph;
pub mod inspect;
pub mod mail;
pub mod media;
pub mod memory;
pub mod office;
pub mod plan;
pub mod policy;
pub mod pricing;
pub mod project;
pub mod providers;
pub mod repos;
pub mod runlog;
pub mod secrets;
pub mod settings;
pub mod symbols;
pub mod syntax;
pub mod syscap;
pub mod tests_select;
pub mod treesitter;
pub mod usage_log;
pub mod verify;
pub mod websearch;
pub mod workflows;
pub mod workspace;

pub use agent::{
    agent_loop_system_prompt, agent_session_path, agent_system_prompt, apply_file_edits,
    audit_log_path, builtin_tools, describe_call, diff_line_stats, diff_stats_by_file,
    execute_tool, git_checkpoint, git_commit_all, git_init, git_log, git_restore, git_revert_all,
    git_review, history_tokens, load_agent_session, parse_file_edits, partial_json_string_field,
    porcelain_path, run_agent, run_shell_command, save_agent_session, system_prompt_for,
    thinking_tail, tools_for, work_system_prompt, AgentEvent, AgentOutcome, AgentSession,
    AppliedEdit, Checkpoint, FileEdit, GitReview, Profile,
};
pub use brand::{ansi_fg as brand_ansi_fg, mix as brand_mix, TAGLINE};
pub use browser::{
    failed_metrics, find_browser, find_metric, missing_content, render_dom, MetricCheck,
};
pub use codenav::{
    find_definitions, find_references, outline, rename_symbol, DefHit, RefHit, RenameResult,
};
pub use config::{load_config, Config, ConfigLoad, PolicyConfig};
pub use context::{
    assemble_context_prompt, build_context_pack, build_context_pack_for_query, estimate_tokens,
    ContextEntry, ContextPack,
};
pub use diagnostics::{checker_name, run_diagnostics, Diagnostic, Severity};
pub use docwrite::{
    column_name, markdown_to_document_xml, write_docx, write_workbook, write_xlsx, Chart,
    ChartKind, Sheet,
};
pub use environment::{discover_environment, EnvironmentReport, ToolInfo, WslInfo};
pub use format::{can_format, format_source, formatter_for, Formatter};
pub use graph::{
    build_graph_from_files, build_project_graph, DependencyEdge, FileNode, ProjectGraph,
};
pub use inspect::{
    inspect_project, project_symbols, CommandKind, CommandSuggestion, FileInventory,
    LanguageSummary, ProjectInspection, ProjectMarker, SymbolSummary,
};
pub use mail::{draft_message, list_inbox, read_message, send_message, MailSummary};
pub use media::{
    base64_encode, classify, document_context, load_image, AttachmentKind, ImageAttachment,
};
pub use memory::{load_memory, remember, render_memory, save_memory, MemoryNote};
pub use office::{kind_for, read_document, DocKind};
pub use plan::{
    clear_plan, load_plan, plan_from_tool_input, save_plan, Plan, PlanStep, StepStatus,
};
pub use policy::{default_denied_patterns, effective_policy, Policy};
pub use pricing::{cost_of_usage, estimate_cost, model_context_window, model_price, ModelPrice};
pub use project::{create_project, push_recent, NewProject, MAX_RECENTS};
pub use providers::{
    chat, chat_stream, run_turn, run_turn_streaming, AgentMessage, ChatMessage, ChatRequest,
    ProviderConfig, ProviderKind, ToolCall, ToolResult, ToolSpec, TurnEvent, TurnResult, Usage,
};
pub use repos::{
    link_repo, load_workspace, resolve_repo, save_workspace, unlink_repo, Repo, Workspace,
};
pub use runlog::{is_tool_output, summarize_output, Status, Summary};
pub use secrets::{scan_secrets, SecretFinding};
pub use settings::{
    load_settings, model_suggestions, model_suggestions_for, provider_preset, save_settings,
    Budget, ProviderSettings, Settings, UserInfo, PROVIDER_PRESETS,
};
pub use symbols::{
    extractor_for_language, extractor_for_path, symbols_for_file, FileSymbols, Import, Symbol,
    SymbolExtractor, SymbolKind,
};
pub use syntax::{highlight, language_from_extension, Language, Span, TokenKind};
pub use syscap::{
    app_logs, detect_url, http_check, is_long_running, list_screenshots, open_path, open_url,
    running_apps, start_app_detached, stop_app, take_screenshot, task_state_from, task_status,
    RunningApp, TaskState,
};
pub use tests_select::{is_test_path, select_tests, TestSelection};
pub use usage_log::{
    append_usage_record, cost_since, cost_today, format_ts, load_usage_records, now_epoch,
    record_savings, start_of_day_utc, summarize_usage, usage_csv, UsageRecord, UsageSummary,
    UsageTotals,
};
pub use verify::{plan_verification, run_verification, StepResult, VerificationReport, VerifyStep};
pub use websearch::{web_search, SearchResult};
pub use workflows::{
    all_workflows, builtin_workflows, catalog_workflows, export_workflows_to,
    import_workflows_from, install_workflow, is_builtin_workflow, load_user_workflows,
    materialize_resources, remove_user_workflow, save_user_workflows, workflows_path, Workflow,
    WorkflowResource,
};
pub use workspace::{
    create_dir, create_file, delete_entry, read_dir_entries, read_text_file, rename_entry,
    validate_entry_name, write_text_file, WorkspaceEntry,
};
