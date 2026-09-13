mod buffer;
use std::sync::Arc;
mod syntax_map;
use async_trait::async_trait;
pub use buffer::*;
mod manifest;
mod toolchain;
use collections::HashMap;
use futures::lock::OwnedMutexGuard;
use gpui::{App, AsyncApp};
use language_core::{
    CodeLabel, Diagnostic, Grammar, InjectionConfig, LanguageConfig, LanguageId, Symbol,
};
use lsp::{CodeActionKind, InitializeParams, LanguageServerBinary, Uri};
pub use manifest::{ManifestDelegate, ManifestName, ManifestProvider, ManifestQuery};
pub use toolchain::{
    LanguageToolchainStore, LocalLanguageToolchainStore, Toolchain, ToolchainList, ToolchainLister,
    ToolchainMetadata, ToolchainScope,
};
// use syntax_map::{QueryCursorHandle, SyntaxSnapshot};
pub struct Language {
    pub(crate) id: LanguageId,
    pub(crate) config: LanguageConfig,
    pub(crate) grammar: Option<Arc<Grammar>>,
    // pub(crate) context_provider: Option<Arc<dyn ContextProvider>>,
    // pub(crate) toolchain: Option<Arc<dyn ToolchainLister>>,
    pub(crate) manifest_name: Option<ManifestName>,
}
// pub use syntax_map::{
//     OwnedSyntaxLayer, SyntaxLayer, SyntaxMapMatches, ToTreeSitterPoint, TreeSitterOptions,
// };
mod language_registry;

pub use language_registry::{
    LanguageName, LanguageServerStatusUpdate, LoadedLanguage, ServerHealth,
};
pub use lsp::{LanguageServerId, LanguageServerName};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Represents a Language Server, with certain cached sync properties.
/// Uses [`LspAdapter`] under the hood, but calls all 'static' methods
/// once at startup, and caches the results.
pub struct CachedLspAdapter {
    pub name: LanguageServerName,
    pub disk_based_diagnostic_sources: Vec<String>,
    pub disk_based_diagnostics_progress_token: Option<String>,
    language_ids: HashMap<LanguageName, String>,
    pub adapter: Arc<dyn LspAdapter>,
    cached_binary: Arc<ServerBinaryCache>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default, JsonSchema)]
pub struct LanguageMatcher {
    /// Given a list of `LanguageConfig`'s, the language of a file can be determined based on the path extension matching any of the `path_suffixes`.
    #[serde(default)]
    pub path_suffixes: Vec<String>,
    /// A regex pattern that determines whether the language should be assigned to a file or not.
    #[serde(
        default,
        serialize_with = "serialize_regex",
        deserialize_with = "deserialize_regex"
    )]
    #[schemars(schema_with = "regex_json_schema")]
    pub first_line_pattern: Option<Regex>,
    /// Alternative names for this language used in vim/emacs modelines.
    /// These are matched case-insensitively against the `mode` (emacs) or
    /// `filetype`/`ft` (vim) specified in the modeline.
    #[serde(default)]
    pub modeline_aliases: Vec<String>,
}

impl Ord for LanguageMatcher {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path_suffixes
            .cmp(&other.path_suffixes)
            .then_with(|| {
                self.first_line_pattern
                    .as_ref()
                    .map(Regex::as_str)
                    .cmp(&other.first_line_pattern.as_ref().map(Regex::as_str))
            })
            .then_with(|| self.modeline_aliases.cmp(&other.modeline_aliases))
    }
}

impl PartialOrd for LanguageMatcher {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

type ServerBinaryCache = futures::lock::Mutex<Option<(bool, LanguageServerBinary)>>;

/// Commands that the client (editor) handles locally rather than forwarding
/// to the language server. Servers embed these in code lens and code action
/// responses when they want the editor to perform a well-known UI action.
#[derive(Debug, Clone)]
pub enum ClientCommand {
    /// Open a location list (references panel / peek view).
    ShowLocations,
    /// Schedule a task from an LSP command's arguments.
    ScheduleTask(task::TaskTemplate),
}

/// A shared grammar for plain text, exposed for reuse by downstream crates.
pub static PLAIN_TEXT: LazyLock<Arc<Language>> = LazyLock::new(|| {
    Arc::new(Language::new(
        LanguageConfig {
            name: "Plain Text".into(),
            soft_wrap: Some(SoftWrap::EditorWidth),
            matcher: LanguageMatcher {
                path_suffixes: vec!["txt".to_owned()],
                first_line_pattern: None,
                modeline_aliases: vec!["text".to_owned(), "txt".to_owned()],
            },
            brackets: BracketPairConfig {
                pairs: vec![
                    BracketPair {
                        start: "(".to_string(),
                        end: ")".to_string(),
                        close: true,
                        surround: true,
                        newline: false,
                    },
                    BracketPair {
                        start: "[".to_string(),
                        end: "]".to_string(),
                        close: true,
                        surround: true,
                        newline: false,
                    },
                    BracketPair {
                        start: "{".to_string(),
                        end: "}".to_string(),
                        close: true,
                        surround: true,
                        newline: false,
                    },
                    BracketPair {
                        start: "\"".to_string(),
                        end: "\"".to_string(),
                        close: true,
                        surround: true,
                        newline: false,
                    },
                    BracketPair {
                        start: "'".to_string(),
                        end: "'".to_string(),
                        close: true,
                        surround: true,
                        newline: false,
                    },
                ],
                disabled_scopes_by_bracket_ix: Default::default(),
            },
            ..Default::default()
        },
        None,
    ))
});

#[async_trait(?Send)]
pub trait LspAdapter: 'static + Send + Sync + DynLspInstaller {
    fn name(&self) -> LanguageServerName;

    fn process_diagnostics(&self, _: &mut lsp::PublishDiagnosticsParams, _: LanguageServerId) {}

    /// When processing new `lsp::PublishDiagnosticsParams` diagnostics, whether to retain previous one(s) or not.
    fn retain_old_diagnostic(&self, _previous_diagnostic: &Diagnostic) -> bool {
        false
    }

    /// Whether to underline a given diagnostic or not, when rendering in the editor.
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#diagnosticTag
    /// states that
    /// > Clients are allowed to render diagnostics with this tag faded out instead of having an error squiggle.
    /// for the unnecessary diagnostics, so do not underline them.
    fn underline_diagnostic(&self, _diagnostic: &lsp::Diagnostic) -> bool {
        true
    }

    /// Post-processes completions provided by the language server.
    async fn process_completions(&self, _: &mut [lsp::CompletionItem]) {}

    fn diagnostic_message_to_markdown(&self, _message: &str) -> Option<String> {
        None
    }

    async fn labels_for_completions(
        self: Arc<Self>,
        completions: &[lsp::CompletionItem],
        language: &Arc<Language>,
    ) -> Result<Vec<Option<CodeLabel>>> {
        let mut labels = Vec::new();
        for (ix, completion) in completions.iter().enumerate() {
            let label = self.label_for_completion(completion, language).await;
            if let Some(label) = label {
                labels.resize(ix + 1, None);
                *labels.last_mut().unwrap() = Some(label);
            }
        }
        Ok(labels)
    }

    async fn label_for_completion(
        &self,
        _: &lsp::CompletionItem,
        _: &Arc<Language>,
    ) -> Option<CodeLabel> {
        None
    }

    async fn labels_for_symbols(
        self: Arc<Self>,
        symbols: &[Symbol],
        language: &Arc<Language>,
    ) -> Result<Vec<Option<CodeLabel>>> {
        let mut labels = Vec::new();
        for (ix, symbol) in symbols.iter().enumerate() {
            let label = self.label_for_symbol(symbol, language).await;
            if let Some(label) = label {
                labels.resize(ix + 1, None);
                *labels.last_mut().unwrap() = Some(label);
            }
        }
        Ok(labels)
    }

    async fn label_for_symbol(
        &self,
        _symbol: &Symbol,
        _language: &Arc<Language>,
    ) -> Option<CodeLabel> {
        None
    }

    /// Returns initialization options that are going to be sent to a LSP server as a part of [`lsp::InitializeParams`]
    async fn initialization_options(
        self: Arc<Self>,
        _: &Arc<dyn LspAdapterDelegate>,
        _cx: &mut AsyncApp,
    ) -> Result<Option<Value>> {
        Ok(None)
    }

    /// Returns the JSON schema of the initialization_options for the language server.
    async fn initialization_options_schema(
        self: Arc<Self>,
        _delegate: &Arc<dyn LspAdapterDelegate>,
        _cached_binary: OwnedMutexGuard<Option<(bool, LanguageServerBinary)>>,
        _cx: &mut AsyncApp,
    ) -> Option<serde_json::Value> {
        None
    }

    /// Returns the JSON schema of the settings for the language server.
    /// This corresponds to the `settings` field in `LspSettings`, which is used
    /// to respond to `workspace/configuration` requests from the language server.
    async fn settings_schema(
        self: Arc<Self>,
        _delegate: &Arc<dyn LspAdapterDelegate>,
        _cached_binary: OwnedMutexGuard<Option<(bool, LanguageServerBinary)>>,
        _cx: &mut AsyncApp,
    ) -> Option<serde_json::Value> {
        None
    }

    async fn workspace_configuration(
        self: Arc<Self>,
        _: &Arc<dyn LspAdapterDelegate>,
        _: Option<Toolchain>,
        _: Option<Uri>,
        _cx: &mut AsyncApp,
    ) -> Result<Value> {
        Ok(serde_json::json!({}))
    }

    async fn additional_initialization_options(
        self: Arc<Self>,
        _target_language_server_id: LanguageServerName,
        _: &Arc<dyn LspAdapterDelegate>,
    ) -> Result<Option<Value>> {
        Ok(None)
    }

    async fn additional_workspace_configuration(
        self: Arc<Self>,
        _target_language_server_id: LanguageServerName,
        _: &Arc<dyn LspAdapterDelegate>,
        _cx: &mut AsyncApp,
    ) -> Result<Option<Value>> {
        Ok(None)
    }

    /// Returns a list of code actions supported by a given LspAdapter
    fn code_action_kinds(&self) -> Option<Vec<CodeActionKind>> {
        None
    }

    fn disk_based_diagnostic_sources(&self) -> Vec<String> {
        Default::default()
    }

    fn disk_based_diagnostics_progress_token(&self) -> Option<String> {
        None
    }

    fn language_ids(&self) -> HashMap<LanguageName, String> {
        HashMap::default()
    }

    /// Support custom initialize params.
    fn prepare_initialize_params(
        &self,
        original: InitializeParams,
        _: &App,
    ) -> Result<InitializeParams> {
        Ok(original)
    }

    fn client_command(
        &self,
        _command_name: &str,
        _arguments: &[serde_json::Value],
    ) -> Option<ClientCommand> {
        None
    }

    /// Method only implemented by the default JSON language server adapter.
    /// Used to provide dynamic reloading of the JSON schemas used to
    /// provide autocompletion and diagnostics in Zed setting and keybind
    /// files
    fn is_primary_zed_json_schema_adapter(&self) -> bool {
        false
    }

    /// True for the extension adapter and false otherwise.
    fn is_extension(&self) -> bool {
        false
    }

    /// Called when a user responds to a ShowMessageRequest from this language server.
    /// This allows adapters to intercept preference selections (like "Always" or "Never")
    /// for settings that should be persisted to Zed's settings file.
    fn process_prompt_response(&self, _context: &PromptResponseContext, _cx: &mut AsyncApp) {}
}

#[async_trait(?Send)]
pub trait DynLspInstaller {
    async fn try_fetch_server_binary(
        &self,
        delegate: &Arc<dyn LspAdapterDelegate>,
        container_dir: PathBuf,
        pre_release: bool,
        cx: &mut AsyncApp,
    ) -> Result<LanguageServerBinary>;

    fn get_language_server_command(
        self: Arc<Self>,
        delegate: Arc<dyn LspAdapterDelegate>,
        toolchains: Option<Toolchain>,
        binary_options: LanguageServerBinaryOptions,
        cached_binary: OwnedMutexGuard<Option<(bool, LanguageServerBinary)>>,
        cx: AsyncApp,
    ) -> LanguageServerBinaryLocations;
}

/// [`LspAdapterDelegate`] allows [`LspAdapter]` implementations to interface with the application
// e.g. to display a notification or fetch data from the web.
#[async_trait]
pub trait LspAdapterDelegate: Send + Sync {
    fn show_notification(&self, message: &str, cx: &mut App);
    fn http_client(&self) -> Arc<dyn HttpClient>;
    fn worktree_id(&self) -> WorktreeId;
    fn worktree_root_path(&self) -> &Path;
    fn resolve_relative_path(&self, path: PathBuf) -> PathBuf;
    fn update_status(&self, language: LanguageServerName, status: BinaryStatus);
    fn registered_lsp_adapters(&self) -> Vec<Arc<dyn LspAdapter>>;
    async fn language_server_download_dir(&self, name: &LanguageServerName) -> Option<Arc<Path>>;

    async fn npm_package_installed_version(
        &self,
        package_name: &str,
    ) -> Result<Option<(PathBuf, Version)>>;
    async fn which(&self, command: &OsStr) -> Option<PathBuf>;
    async fn shell_env(&self) -> HashMap<String, String>;
    async fn read_text_file(&self, path: &RelPath) -> Result<String>;
    async fn try_exec(&self, binary: LanguageServerBinary) -> Result<()>;
}
