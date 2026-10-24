
use crate::store::{Store, StoreError};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    And,
    Or,
    AndNot,
    NotAnd,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    #[default]
    Unordered,
    Ordered,
    Fuzzy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchDomain {
    #[default]
    Names,
    Functions,
    RawContent,
}

/// Normalize the two supported path separators.
fn normalize(method: &str) -> String {
    method.replace("--", "/")
}

/// Match a name against a method path.
fn matches_name(method: &str, name: &str, mode: MatchMode) -> bool {
    if name.is_empty() {
        return false;
    }

    let normalized = normalize(method);
    let parts: Vec<&str> = normalized.split('/').collect();

    match mode {
        MatchMode::Ordered => {
            parts.first() == Some(&name)
                || (parts.first() == Some(&"f") && parts.get(2) == Some(&name))
        }
        MatchMode::Unordered | MatchMode::Fuzzy => parts.contains(&name),
    }
}

/// Match a function name against `f/<name>/...`.
fn matches_function(method: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    let normalized = normalize(method);
    let parts: Vec<&str> = normalized.split('/').collect();

    parts.first() == Some(&"f") && parts.get(1) == Some(&name)
}

/// Convert text into lowercase alphanumeric words.
fn words(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            current.extend(ch.to_lowercase());
        } else if !current.is_empty() {
            result.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        result.push(current);
    }

    result
}

/// Levenshtein edit distance, measured in Unicode characters.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();

    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];

    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;

        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            let insertion = current[j] + 1;
            let deletion = previous[j + 1] + 1;

            current[j + 1] = substitution.min(insertion).min(deletion);
        }

        std::mem::swap(&mut previous, &mut current);
    }

    previous[b.len()]
}

/// Allow one typo in medium words and two in longer words.
fn edit_tolerance(word: &str) -> usize {
    match word.chars().count() {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    }
}

/// Fuzzy search for all query words in the raw content.
fn fuzzy_contains(raw_content: &str, query: &str) -> bool {
    if query.trim().is_empty() {
        return false;
    }

    if raw_content.to_lowercase().contains(&query.to_lowercase()) {
        return true;
    }

    let query_words = words(query);
    let content_words = words(raw_content);

    !query_words.is_empty()
        && query_words.iter().all(|query_word| {
            let tolerance = edit_tolerance(query_word);

            content_words.iter().any(|content_word| {
                content_word == query_word
                    || (tolerance > 0
                        && content_word.chars().count().abs_diff(
                            query_word.chars().count(),
                        ) <= tolerance
                        && edit_distance(content_word, query_word) <= tolerance)
            })
        })
}

/// Search atom hashes by an arbitrary list of names.
///
/// `AndNot`: contains the first name and none of the remaining names.
/// `NotAnd`: contains none of the supplied names.
///
/// `Fuzzy` mode applies to `RawContent`. For other domains it behaves
/// like unordered matching.
pub fn search(
    store: &Store,
    names: &[String],
    operation: Operation,
    mode: MatchMode,
    domain: SearchDomain,
) -> Result<Vec<String>, StoreError> {
    if names.is_empty() || names.iter().all(|name| name.trim().is_empty()) {
        return Ok(Vec::new());
    }

    let mut stmt = store.conn.prepare_cached(
        "SELECT atoms.id, atoms.raw_content, atom_methods.method
         FROM atoms
         LEFT JOIN atom_methods ON atom_methods.atom_id = atoms.id
         ORDER BY atoms.id",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;

    let mut atoms: BTreeMap<Vec<u8>, (String, Vec<String>)> = BTreeMap::new();

    for row in rows {
        let (id, raw_content, method) = row?;
        let entry = atoms.entry(id).or_insert_with(|| (raw_content, Vec::new()));

        if let Some(method) = method {
            entry.1.push(method);
        }
    }

    let mut result = BTreeSet::new();

    for (id, (raw_content, methods)) in atoms {
        let contains: Vec<bool> = names
            .iter()
            .map(|name| match domain {
                SearchDomain::Names => methods
                    .iter()
                    .any(|method| matches_name(method, name, mode)),
                SearchDomain::Functions => methods
                    .iter()
                    .any(|method| matches_function(method, name)),
                SearchDomain::RawContent => match mode {
                    MatchMode::Fuzzy => fuzzy_contains(&raw_content, name),
                    MatchMode::Unordered | MatchMode::Ordered => {
                        !name.trim().is_empty()
                            && raw_content
                                .to_lowercase()
                                .contains(&name.to_lowercase())
                    }
                },
            })
            .collect();

        let matched = match operation {
            Operation::And => contains.iter().all(|matched| *matched),
            Operation::Or => contains.iter().any(|matched| *matched),
            Operation::AndNot => {
                contains.first() == Some(&true)
                    && contains.iter().skip(1).all(|matched| !matched)
            }
            Operation::NotAnd => contains.iter().all(|matched| !matched),
        };

        if matched {
            result.insert(hex::encode(id));
        }
    }

    Ok(result.into_iter().collect())
}

/// Find atom hashes whose methods contain `name` as a path component.
pub fn hashes_by_name(
    store: &Store,
    name: &str,
) -> Result<Vec<String>, StoreError> {
    search(
        store,
        &[name.to_string()],
        Operation::Or,
        MatchMode::Unordered,
        SearchDomain::Names,
    )
}

/// Find atom hashes with a method beginning with `f/<name>`.
pub fn hashes_by_function(
    store: &Store,
    name: &str,
) -> Result<Vec<String>, StoreError> {
    let prefix = format!("f/{name}");

    let mut stmt = store.conn.prepare_cached(
        "SELECT DISTINCT atom_id
         FROM atom_methods
         WHERE method = ?1
            OR method LIKE ?2 ESCAPE '\\'
         ORDER BY atom_id",
    )?;

    // Escape LIKE metacharacters so the name is treated literally.
    let escaped = prefix
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");

    let pattern = format!("{escaped}/%");

    let ids = stmt
        .query_map([&prefix, &pattern], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ids.into_iter().map(hex::encode).collect())
}
