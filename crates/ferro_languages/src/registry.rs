//! The registry of Language Packs: built-in ones and packs from a folder.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use ferro_syntax::{LanguageConfig, LanguageResolver, QueryKind, SyntaxLanguage};
use regex::Regex;

use crate::{
    detect,
    grammars::builtin_grammar,
    manifest::{GrammarSource, LanguageManifest},
};

/// Why a pack could not be loaded or its grammar built.
#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("{path}: {error}")]
    Manifest { path: String, error: String },
    #[error("{0}: no grammar")]
    NoGrammar(String),
    #[error("{pack}: unknown built-in grammar {grammar:?}")]
    UnknownGrammar { pack: String, grammar: String },
    #[error("{pack}: WebAssembly grammars ({file}) need the extension host (Stage 11)")]
    WasmUnsupported { pack: String, file: String },
    #[error("{pack}: grammar from {git} must be built into a WebAssembly file first")]
    GitUnsupported { pack: String, git: String },
    #[error(transparent)]
    Query(#[from] ferro_syntax::LanguageError),
    #[error("{0}")]
    Io(String),
}

/// One Language Pack: its manifest, its query files and — built on first use
/// — its grammar with compiled queries.
pub struct LanguagePack {
    manifest: LanguageManifest,
    /// The pack's folder for packs loaded from disk.
    dir: Option<PathBuf>,
    /// Query files of the pack itself, by file name.
    files: Vec<(String, String)>,
    first_line: Option<Regex>,
    syntax: OnceLock<Result<Arc<SyntaxLanguage>, String>>,
    builds: Arc<AtomicUsize>,
}

impl std::fmt::Debug for LanguagePack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanguagePack")
            .field("id", &self.manifest.id)
            .finish_non_exhaustive()
    }
}

impl LanguagePack {
    fn new(
        manifest: LanguageManifest,
        dir: Option<PathBuf>,
        files: Vec<(String, String)>,
        builds: Arc<AtomicUsize>,
    ) -> Self {
        let first_line =
            manifest
                .first_line
                .as_deref()
                .and_then(|pattern| match Regex::new(pattern) {
                    Ok(regex) => Some(regex),
                    Err(err) => {
                        tracing::warn!("{}: bad first_line pattern: {err}", manifest.id);
                        None
                    }
                });
        Self {
            manifest,
            dir,
            files,
            first_line,
            syntax: OnceLock::new(),
            builds,
        }
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    pub fn manifest(&self) -> &LanguageManifest {
        &self.manifest
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Whether the pack's grammar and queries were built already.
    pub fn is_loaded(&self) -> bool {
        self.syntax.get().is_some()
    }

    /// The source of the pack's queries of one kind: its own file, or the
    /// grammar's bundled query when the pack has no file of that name.
    pub fn query_source(&self, kind: QueryKind) -> String {
        let file = kind.file_name();
        let listed = self
            .manifest
            .grammar
            .as_ref()
            .is_some_and(|g| g.queries.is_empty() || g.queries.iter().any(|q| q == file));
        if !listed {
            return String::new();
        }
        if let Some((_, source)) = self.files.iter().find(|(name, _)| name == file) {
            // `; inherits: grammar` keeps the grammar's query and adds to it.
            if source.trim_start().starts_with("; inherits: grammar") {
                return format!("{}\n{}", source, self.bundled(file));
            }
            return source.clone();
        }
        self.bundled(file)
    }

    fn bundled(&self, file: &str) -> String {
        let Some(grammar) = self
            .manifest
            .grammar
            .as_ref()
            .and_then(|g| builtin_grammar(&g.name))
        else {
            return String::new();
        };
        grammar
            .queries
            .iter()
            .filter(|(name, _)| *name == file)
            .map(|(_, source)| *source)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The grammar of the pack, without compiling queries.
    pub fn grammar(&self) -> Result<tree_sitter::Language, PackError> {
        let grammar = self
            .manifest
            .grammar
            .as_ref()
            .ok_or_else(|| PackError::NoGrammar(self.manifest.id.clone()))?;
        match &grammar.source {
            GrammarSource::Builtin => builtin_grammar(&grammar.name)
                .map(|g| (g.language)())
                .ok_or_else(|| PackError::UnknownGrammar {
                    pack: self.manifest.id.clone(),
                    grammar: grammar.name.clone(),
                }),
            GrammarSource::Wasm { wasm } => Err(PackError::WasmUnsupported {
                pack: self.manifest.id.clone(),
                file: wasm.clone(),
            }),
            GrammarSource::Git { git, .. } => builtin_grammar(&grammar.name)
                .map(|g| (g.language)())
                .ok_or_else(|| PackError::GitUnsupported {
                    pack: self.manifest.id.clone(),
                    git: git.clone(),
                }),
        }
    }

    /// The pack's grammar with its compiled queries, built on first call.
    pub fn syntax(&self) -> Result<Arc<SyntaxLanguage>, String> {
        self.syntax
            .get_or_init(|| {
                self.builds.fetch_add(1, Ordering::Relaxed);
                let started = std::time::Instant::now();
                let result = self.build().map(Arc::new).map_err(|e| e.to_string());
                match &result {
                    Ok(_) => tracing::debug!(
                        "language {} loaded in {:.1} ms",
                        self.id(),
                        started.elapsed().as_secs_f64() * 1000.0
                    ),
                    Err(err) => tracing::warn!("language {}: {err}", self.id()),
                }
                result
            })
            .clone()
    }

    fn build(&self) -> Result<SyntaxLanguage, PackError> {
        let grammar = self.grammar()?;
        let queries = QueryKind::ALL
            .into_iter()
            .map(|kind| (kind, self.query_source(kind)))
            .filter(|(_, source)| !source.trim().is_empty())
            .collect();
        Ok(SyntaxLanguage::new(LanguageConfig {
            id: self.manifest.id.clone(),
            grammar,
            queries,
            brackets: self
                .manifest
                .brackets
                .iter()
                .map(|[a, b]| (a.clone(), b.clone()))
                .collect(),
            indent_unit: self.manifest.indent.unit.clone(),
        })?)
    }
}

/// Every known Language Pack.
pub struct LanguageRegistry {
    packs: Vec<Arc<LanguagePack>>,
    /// Lower-case id or alias → index.
    by_name: HashMap<String, usize>,
    /// Lower-case extension → index.
    by_extension: HashMap<String, usize>,
    by_file_name: HashMap<String, usize>,
    builds: Arc<AtomicUsize>,
}

impl std::fmt::Debug for LanguageRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.packs.iter().map(|p| p.id()))
            .finish()
    }
}

static BUILTIN: LazyLock<Arc<LanguageRegistry>> =
    LazyLock::new(|| Arc::new(LanguageRegistry::builtin()));

impl LanguageRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            packs: Vec::new(),
            by_name: HashMap::new(),
            by_extension: HashMap::new(),
            by_file_name: HashMap::new(),
            builds: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The packs that ship with FerroIDE (`languages/` in the repository).
    pub fn builtin() -> Self {
        let mut registry = Self::new();
        for (id, files) in crate::builtin::packs() {
            let Some((_, manifest)) = files.iter().find(|(name, _)| *name == "language.toml")
            else {
                continue;
            };
            match LanguageManifest::parse(manifest) {
                Ok(manifest) => {
                    let files = files
                        .iter()
                        .filter(|(name, _)| name.ends_with(".scm"))
                        .map(|(name, source)| (name.to_string(), source.to_string()))
                        .collect();
                    registry.add(manifest, None, files);
                }
                // Built-in manifests are checked by tests.
                Err(err) => tracing::error!("built-in pack {id}: {err}"),
            }
        }
        registry
    }

    /// A shared registry of the built-in packs.
    pub fn shared_builtin() -> Arc<LanguageRegistry> {
        BUILTIN.clone()
    }

    /// Add a pack, replacing a pack with the same id.
    pub fn add(
        &mut self,
        manifest: LanguageManifest,
        dir: Option<PathBuf>,
        files: Vec<(String, String)>,
    ) {
        let pack = Arc::new(LanguagePack::new(manifest, dir, files, self.builds.clone()));
        let index = match self.packs.iter().position(|p| p.id() == pack.id()) {
            Some(index) => {
                self.packs[index] = pack;
                index
            }
            None => {
                self.packs.push(pack);
                self.packs.len() - 1
            }
        };
        let manifest = self.packs[index].manifest.clone();
        for name in manifest.names() {
            self.by_name.insert(name, index);
        }
        if !manifest.hidden {
            for ext in &manifest.file_extensions {
                self.by_extension.insert(ext.to_lowercase(), index);
            }
            for name in &manifest.file_names {
                self.by_file_name.insert(name.clone(), index);
            }
        }
    }

    /// Load every pack in `dir` (one folder per pack, each with a
    /// `language.toml`), replacing built-in packs of the same id. Returns the
    /// packs that failed to load.
    pub fn load_dir(&mut self, dir: &Path) -> Vec<PackError> {
        let mut errors = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return errors;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("language.toml").is_file())
            .collect();
        dirs.sort();
        for pack_dir in dirs {
            match load_pack(&pack_dir) {
                Ok((manifest, files)) => self.add(manifest, Some(pack_dir), files),
                Err(err) => errors.push(err),
            }
        }
        errors
    }

    pub fn packs(&self) -> impl Iterator<Item = &Arc<LanguagePack>> {
        self.packs.iter()
    }

    /// Packs shown to users (not hidden helper grammars).
    pub fn visible_packs(&self) -> impl Iterator<Item = &Arc<LanguagePack>> {
        self.packs.iter().filter(|p| !p.manifest.hidden)
    }

    /// The pack with this id or alias (case-insensitive).
    pub fn get(&self, name: &str) -> Option<&Arc<LanguagePack>> {
        self.by_name
            .get(&name.to_lowercase())
            .map(|&index| &self.packs[index])
    }

    /// How many packs had their grammar built so far.
    pub fn loaded_count(&self) -> usize {
        self.builds.load(Ordering::Relaxed)
    }

    /// The language of a file from its path alone: file name, then the
    /// longest matching extension.
    pub fn detect_path(&self, path: &Path) -> Option<&Arc<LanguagePack>> {
        let file_name = path.file_name()?.to_str()?;
        if let Some(&index) = self.by_file_name.get(file_name) {
            return Some(&self.packs[index]);
        }
        let lower = file_name.to_lowercase();
        // `foo.d.ts` tries `d.ts` before `ts`.
        let mut rest = lower.as_str();
        while let Some(dot) = rest.find('.') {
            rest = &rest[dot + 1..];
            if let Some(&index) = self.by_extension.get(rest) {
                return Some(&self.packs[index]);
            }
        }
        None
    }

    /// The language of a file. A modeline in `head` (the first lines of the
    /// file) wins; then the file name and extension; then the shebang and the
    /// packs' `first_line` patterns.
    pub fn detect(&self, path: &Path, head: Option<&str>) -> Option<&Arc<LanguagePack>> {
        if let Some(head) = head
            && let Some(name) = detect::modeline_language(head)
            && let Some(pack) = self.get(&name).filter(|p| !p.manifest.hidden)
        {
            return Some(pack);
        }
        if let Some(pack) = self.detect_path(path) {
            return Some(pack);
        }
        let first = head?.lines().next()?;
        if let Some(interpreter) = detect::shebang_interpreter(first) {
            return self.visible_packs().find(|p| {
                p.manifest
                    .shebangs
                    .iter()
                    .any(|entry| detect::interpreter_matches(interpreter, entry))
            });
        }
        self.visible_packs()
            .find(|p| p.first_line.as_ref().is_some_and(|re| re.is_match(first)))
    }

    /// A resolver for injections: languages by id or alias, built on demand.
    pub fn resolver(self: &Arc<Self>) -> Arc<dyn LanguageResolver> {
        Arc::new(RegistryResolver(self.clone()))
    }
}

impl Default for LanguageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

struct RegistryResolver(Arc<LanguageRegistry>);

impl LanguageResolver for RegistryResolver {
    fn resolve(&self, name: &str) -> Option<Arc<SyntaxLanguage>> {
        self.0.get(name)?.syntax().ok()
    }
}

fn load_pack(dir: &Path) -> Result<(LanguageManifest, Vec<(String, String)>), PackError> {
    let manifest_path = dir.join("language.toml");
    let source = fs::read_to_string(&manifest_path)
        .map_err(|e| PackError::Io(format!("{}: {e}", manifest_path.display())))?;
    let manifest = LanguageManifest::parse(&source).map_err(|e| PackError::Manifest {
        path: manifest_path.display().to_string(),
        error: e.to_string(),
    })?;
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)
        .map_err(|e| PackError::Io(e.to_string()))?
        .flatten()
    {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "scm")
            && let (Some(name), Ok(text)) = (
                path.file_name().and_then(|n| n.to_str()),
                fs::read_to_string(&path),
            )
        {
            files.push((name.to_string(), text));
        }
    }
    Ok((manifest, files))
}
