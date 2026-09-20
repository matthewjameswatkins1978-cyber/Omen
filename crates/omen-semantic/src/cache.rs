use crate::types::*;
use omen_core::ValidityState;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// A witness recording the exact physical state of a source file at observation time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticWitness {
    pub relative_path: String,
    pub content_hash: String,
    pub modified: Option<SystemTime>,
}

impl SemanticWitness {
    pub fn observe(workspace_root: &Path, relative_path: impl Into<String>) -> Option<Self> {
        let rel = relative_path.into();
        let full = workspace_root.join(&rel);
        let bytes = fs::read(&full).ok()?;
        let modified = fs::metadata(&full).ok().and_then(|m| m.modified().ok());

        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let content_hash = hex::encode(hasher.finalize());

        Some(Self {
            relative_path: rel,
            content_hash,
            modified,
        })
    }

    /// Verifies whether the witness is still valid against the current filesystem.
    pub fn is_current(&self, workspace_root: &Path) -> bool {
        let full = workspace_root.join(&self.relative_path);
        let Ok(bytes) = fs::read(&full) else {
            return false;
        };
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let current_hash = hex::encode(hasher.finalize());
        current_hash == self.content_hash
    }
}

/// A cached semantic value bound to physical witnesses and explicit freshness status.
#[derive(Debug, Clone)]
pub struct CachedEntry<T> {
    pub value: T,
    pub witnesses: Vec<SemanticWitness>,
    pub validity: ValidityState,
    pub generation: SemanticGeneration,
}

impl<T: Clone> CachedEntry<T> {
    pub fn new(value: T, witnesses: Vec<SemanticWitness>, generation: SemanticGeneration) -> Self {
        Self {
            value,
            witnesses,
            validity: ValidityState::Current,
            generation,
        }
    }

    /// Checks witness freshness. If any witness was modified, transitions validity to DIRTY.
    pub fn check_validity(&mut self, workspace_root: &Path) -> ValidityState {
        if self.validity != ValidityState::Current {
            return self.validity;
        }
        for witness in &self.witnesses {
            if !witness.is_current(workspace_root) {
                self.validity = ValidityState::Dirty;
                return ValidityState::Dirty;
            }
        }
        ValidityState::Current
    }
}

/// Thread-safe rebuildable in-memory semantic cache.
#[derive(Debug, Default, Clone)]
pub struct SemanticCache {
    symbols: Arc<Mutex<HashMap<String, CachedEntry<Vec<SymbolRecord>>>>>,
    definitions: Arc<Mutex<HashMap<String, CachedEntry<SourceLocation>>>>,
    references: Arc<Mutex<HashMap<String, CachedEntry<Vec<ReferenceRecord>>>>>,
    workspace_root: PathBuf,
}

impl SemanticCache {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            symbols: Arc::new(Mutex::new(HashMap::new())),
            definitions: Arc::new(Mutex::new(HashMap::new())),
            references: Arc::new(Mutex::new(HashMap::new())),
            workspace_root,
        }
    }

    pub fn insert_symbols(
        &self,
        query: &str,
        records: Vec<SymbolRecord>,
        witnesses: Vec<SemanticWitness>,
        generation: SemanticGeneration,
    ) {
        let entry = CachedEntry::new(records, witnesses, generation);
        self.symbols
            .lock()
            .unwrap()
            .insert(query.to_string(), entry);
    }

    pub fn get_symbols(&self, query: &str) -> Option<(Vec<SymbolRecord>, ValidityState)> {
        let mut map = self.symbols.lock().unwrap();
        let entry = map.get_mut(query)?;
        let validity = entry.check_validity(&self.workspace_root);
        Some((entry.value.clone(), validity))
    }

    pub fn insert_definition(
        &self,
        symbol: &str,
        loc: SourceLocation,
        witnesses: Vec<SemanticWitness>,
        generation: SemanticGeneration,
    ) {
        let entry = CachedEntry::new(loc, witnesses, generation);
        self.definitions
            .lock()
            .unwrap()
            .insert(symbol.to_string(), entry);
    }

    pub fn get_definition(&self, symbol: &str) -> Option<(SourceLocation, ValidityState)> {
        let mut map = self.definitions.lock().unwrap();
        let entry = map.get_mut(symbol)?;
        let validity = entry.check_validity(&self.workspace_root);
        Some((entry.value.clone(), validity))
    }

    pub fn insert_references(
        &self,
        symbol: &str,
        refs: Vec<ReferenceRecord>,
        witnesses: Vec<SemanticWitness>,
        generation: SemanticGeneration,
    ) {
        let entry = CachedEntry::new(refs, witnesses, generation);
        self.references
            .lock()
            .unwrap()
            .insert(symbol.to_string(), entry);
    }

    pub fn get_references(&self, symbol: &str) -> Option<(Vec<ReferenceRecord>, ValidityState)> {
        let mut map = self.references.lock().unwrap();
        let entry = map.get_mut(symbol)?;
        let validity = entry.check_validity(&self.workspace_root);
        Some((entry.value.clone(), validity))
    }

    /// Targeted invalidation when a specific file is changed.
    pub fn invalidate_file(&self, relative_path: &str) {
        let mut syms = self.symbols.lock().unwrap();
        for entry in syms.values_mut() {
            if entry
                .witnesses
                .iter()
                .any(|w| w.relative_path == relative_path)
            {
                entry.validity = ValidityState::Dirty;
            }
        }

        let mut defs = self.definitions.lock().unwrap();
        for entry in defs.values_mut() {
            if entry
                .witnesses
                .iter()
                .any(|w| w.relative_path == relative_path)
            {
                entry.validity = ValidityState::Dirty;
            }
        }

        let mut refs = self.references.lock().unwrap();
        for entry in refs.values_mut() {
            if entry
                .witnesses
                .iter()
                .any(|w| w.relative_path == relative_path)
            {
                entry.validity = ValidityState::Dirty;
            }
        }
    }

    /// Clear all cached derived state.
    pub fn clear(&self) {
        self.symbols.lock().unwrap().clear();
        self.definitions.lock().unwrap().clear();
        self.references.lock().unwrap().clear();
    }
}
