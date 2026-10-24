use crate::methods::extract_methods;
use crate::store::{FoundAtom, SyncReport};
use crate::store::{Store, StoreError};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
pub const META_DIR: &str = ".functorium";
const DB_FILE: &str = "atoms.db";
const CONFIG_FILE: &str = "conf.toml";

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AtomSpec {
    pub del: String,
    pub opendel: String,
    pub closedel: String,
}

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("walk: {0}")]
    Walk(#[from] walkdir::Error),
    #[error("config: {0}")]
    Config(#[from] toml::de::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error("invalid atom-spec: {0}")]
    InvalidAtomSpec(&'static str),
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(rename = "atom-spec")]
    pub atom_spec: AtomSpec,
}

pub struct Journal {
    pub root: PathBuf,
    pub config: Config,
    pub store: Store,
}

#[derive(Debug, PartialEq)]
pub enum Delimiters {
    Single(String),
    Pair { open: String, close: String },
}

impl AtomSpec {
    /// `None` = no delimiters configured.
    pub fn delimiters(&self) -> Result<Option<Delimiters>, JournalError> {
        match (
            self.del.is_empty(),
            self.opendel.is_empty(),
            self.closedel.is_empty(),
        ) {
            (true, true, true) => Ok(None),
            (false, true, true) => Ok(Some(Delimiters::Single(self.del.clone()))),
            (true, false, false) => Ok(Some(Delimiters::Pair {
                open: self.opendel.clone(),
                close: self.closedel.clone(),
            })),
            (true, _, _) => Err(JournalError::InvalidAtomSpec(
                "`opendel` and `closedel` must be set together",
            )),
            _ => Err(JournalError::InvalidAtomSpec(
                "use either `del` or `opendel`/`closedel`, not both",
            )),
        }
    }
}

/// (start, end_including_newline, content_without_line_ending)
fn lines_with_span(text: &str) -> impl Iterator<Item = (usize, usize, &str)> {
    let mut pos = 0;
    text.split_inclusive('\n').map(move |line| {
        let start = pos;
        pos += line.len();
        (start, pos, line.trim_end_matches(['\n', '\r']))
    })
}

pub fn split_atoms<'a>(text: &'a str, delims: Option<&Delimiters>) -> Vec<&'a str> {
    let raw: Vec<&str> = match delims {
        None => vec![text],
        Some(Delimiters::Single(d)) => {
            let mut out = Vec::new();
            let mut atom_start = 0;
            for (start, end, line) in lines_with_span(text) {
                if line == d {
                    out.push(&text[atom_start..start]);
                    atom_start = end;
                }
            }
            out.push(&text[atom_start..]);
            out
        }
        Some(Delimiters::Pair { open, close }) => {
            let mut out = Vec::new();
            let mut body_start: Option<usize> = None;
            for (start, end, line) in lines_with_span(text) {
                match body_start {
                    None if line == open => body_start = Some(end),
                    Some(b) if line == close => {
                        out.push(&text[b..start]);
                        body_start = None;
                    }
                    _ => {}
                }
            }
            out
        }
    };

    raw.into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

impl Journal {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, JournalError> {
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(JournalError::NotADirectory(root));
        }

        let meta = root.join(META_DIR);
        std::fs::create_dir_all(&meta)?;

        let config_path = meta.join(CONFIG_FILE);
        if !config_path.exists() {
            std::fs::write(&config_path, "")?;
        }
        let config: Config = toml::from_str(&std::fs::read_to_string(&config_path)?)?;
        config.atom_spec.delimiters()?;

        let store = Store::open(meta.join(DB_FILE))?;
        store.migrate()?;
        Ok(Self {
            root,
            config,
            store,
        })
    }
    pub fn markdown_files(&self) -> Result<Vec<PathBuf>, JournalError> {
        let walker = WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));

        let mut out = Vec::new();
        for entry in walker {
            let entry = entry?;
            if entry.file_type().is_file() && is_markdown(entry.path()) {
                out.push(entry.path().strip_prefix(&self.root).unwrap().to_path_buf());
            }
        }
        out.sort();
        Ok(out)
    }
    pub fn scan_atoms(&self) -> Result<Vec<FoundAtom>, JournalError> {
        let delims = self.config.atom_spec.delimiters()?;
        let mut found = Vec::new();
        for rel in self.markdown_files()? {
            let text = std::fs::read_to_string(self.root.join(&rel))?;
            for content in split_atoms(&text, delims.as_ref()) {
                found.push(FoundAtom {
                    raw_content: content.to_string(),
                    methods: extract_methods(content)
                        .into_iter()
                        .map(|m| m.text)
                        .collect(),
                });
            }
        }
        Ok(found)
    }

    pub fn sync(&mut self) -> Result<SyncReport, JournalError> {
        let found = self.scan_atoms()?;
        Ok(self.store.sync_atoms(&found)?)
    }
}

fn is_markdown(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}
