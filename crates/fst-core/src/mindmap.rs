use crate::atlas::{self, MatchMode, Operation, SearchDomain};
use crate::store::{Atom, Store, StoreError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Expansion {
    #[default]
    Bounded,
    Unbounded,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    #[default]
    Hierarchical,
    Topological,
    Network,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    TagHierarchy,
    Membership,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Tag,
    Function,
    Atom,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MindmapQuery {
    pub names: Vec<String>,
    pub operation: Operation,
    #[serde(default)]
    pub mode: MatchMode,
    #[serde(default)]
    pub domain: SearchDomain,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GraphOptions {
    pub expansion: Expansion,
    pub split_paths: bool,
    pub include_atoms: bool,
    pub include_functions: bool,
    pub layout: Layout,
    pub edges: Vec<EdgeKind>,
}

impl Default for GraphOptions {
    fn default() -> Self {
        Self {
            expansion: Expansion::Bounded,
            split_paths: true,
            include_atoms: true,
            include_functions: true,
            layout: Layout::Hierarchical,
            edges: vec![EdgeKind::TagHierarchy, EdgeKind::Membership],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MindmapRequest {
    pub query: MindmapQuery,
    #[serde(default)]
    pub graph: GraphOptions,
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub kind: NodeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct Mindmap {
    pub layout: Layout,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

type TagKey = String;

fn normalize(method: &str) -> String {
    method.replace("--", "/").trim_matches('/').to_string()
}

fn is_function(method: &str) -> bool {
    method.strip_prefix("f/").is_some()
}

fn semantic_keys(method: &str) -> Vec<TagKey> {
    let path = normalize(method);
    if path.is_empty() {
        return Vec::new();
    }

    let (prefix, path) = if let Some(rest) = path.strip_prefix("f/") {
        ("function:", rest)
    } else {
        ("tag:", path.as_str())
    };

    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let mut result = Vec::new();
    let mut current = String::new();

    for part in parts {
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(part);
        result.push(format!("{prefix}{current}"));
    }

    result
}

fn atom_keys(atom: &Atom, include_functions: bool) -> BTreeSet<TagKey> {
    atom.methods
        .iter()
        .filter(|method| include_functions || !is_function(method))
        .flat_map(|method| semantic_keys(method))
        .collect()
}

fn node_id(key: &str) -> String {
    key.to_string()
}

fn display_path(method: &str) -> (String, Vec<String>, bool) {
    let path = normalize(method);

    if let Some(rest) = path.strip_prefix("f/") {
        (
            "function:".to_string(),
            rest.split('/')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect(),
            true,
        )
    } else {
        (
            "tag:".to_string(),
            path.split('/')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect(),
            false,
        )
    }
}

fn atom_label(atom: &Atom) -> String {
    let text = if atom.prettier.trim().is_empty() {
        &atom.raw_content
    } else {
        &atom.prettier
    };

    let first_line = text.lines().next().unwrap_or_default().trim();
    if first_line.is_empty() {
        format!("Atom {}", &atom.id[..atom.id.len().min(12)])
    } else if first_line.chars().count() > 120 {
        format!("{}…", first_line.chars().take(117).collect::<String>())
    } else {
        first_line.to_string()
    }
}

fn insert_path(
    method: &str,
    options: &GraphOptions,
    nodes: &mut BTreeMap<String, Node>,
    edges: &mut BTreeSet<Edge>,
) -> Option<String> {
    let (prefix, parts, function) = display_path(method);

    if function && !options.include_functions {
        return None;
    }
    if parts.is_empty() {
        return None;
    }

    let mut current = String::new();
    let mut previous: Option<String> = None;

    for (index, part) in parts.iter().enumerate() {
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(part);

        if !options.split_paths && index + 1 != parts.len() {
            continue;
        }

        let id = format!("{prefix}{current}");
        let is_function_root = function && index == 0;

        let label = match (is_function_root, options.split_paths, function) {
            (true, _, _) => format!("ƒ {part}"),
            (false, true, _) => part.clone(),
            (false, false, true) => format!("ƒ {current}"),
            (false, false, false) => current.clone(),
        };
        nodes.entry(id.clone()).or_insert_with(|| Node {
            id: id.clone(),
            label,
            kind: if is_function_root {
                NodeKind::Function
            } else {
                NodeKind::Tag
            },
            content: None,
        });

        if options.split_paths && options.edges.contains(&EdgeKind::TagHierarchy) {
            if let Some(parent) = &previous {
                edges.insert(Edge {
                    source: parent.clone(),
                    target: id.clone(),
                    kind: EdgeKind::TagHierarchy,
                });
            }
        }

        previous = Some(id);
    }

    previous
}

pub fn build(store: &Store, request: &MindmapRequest) -> Result<Mindmap, StoreError> {
    let matched = atlas::search(
        store,
        &request.query.names,
        request.query.operation,
        request.query.mode,
        request.query.domain,
    )?;

    let mut atoms: BTreeMap<String, Atom> = store
        .all_atoms(None, u32::MAX)?
        .into_iter()
        .map(|atom| (atom.id.clone(), atom))
        .collect();

    let mut selected: BTreeSet<String> = matched.into_iter().collect();

    if request.graph.expansion == Expansion::Unbounded {
        loop {
            let active_keys: BTreeSet<TagKey> = selected
                .iter()
                .filter_map(|id| atoms.get(id))
                .flat_map(|atom| atom_keys(atom, request.graph.include_functions))
                .collect();

            let mut added = Vec::new();

            for (id, atom) in &atoms {
                if selected.contains(id) {
                    continue;
                }

                if !atom_keys(atom, request.graph.include_functions).is_disjoint(&active_keys) {
                    added.push(id.clone());
                }
            }

            if added.is_empty() {
                break;
            }

            selected.extend(added);
        }
    }

    let mut nodes = BTreeMap::<String, Node>::new();
    let mut edges = BTreeSet::<Edge>::new();

    for id in &selected {
        let Some(atom) = atoms.get(id) else {
            continue;
        };

        let atom_node_id = format!("atom:{}", atom.id);

        if request.graph.include_atoms {
            nodes.insert(
                atom_node_id.clone(),
                Node {
                    id: atom_node_id.clone(),
                    label: atom_label(atom),
                    kind: NodeKind::Atom,
                    content: Some(atom.raw_content.clone()),
                },
            );
        }

        for method in &atom.methods {
            let Some(tag_id) = insert_path(method, &request.graph, &mut nodes, &mut edges) else {
                continue;
            };

            if request.graph.include_atoms && request.graph.edges.contains(&EdgeKind::Membership) {
                edges.insert(Edge {
                    source: tag_id,
                    target: atom_node_id.clone(),
                    kind: EdgeKind::Membership,
                });
            }
        }
    }

    // Remove edges whose endpoints were omitted by graph options.
    edges.retain(|edge| nodes.contains_key(&edge.source) && nodes.contains_key(&edge.target));

    Ok(Mindmap {
        layout: request.graph.layout,
        nodes: nodes.into_values().collect(),
        edges: edges.into_iter().collect(),
    })
}

fn mermaid_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\n', " ")
        .replace('\r', " ")
}

pub fn to_mermaid(graph: &Mindmap) -> String {
    let direction = match graph.layout {
        Layout::Network => "LR",
        Layout::Hierarchical | Layout::Topological => "TD",
    };

    let mut output = format!("flowchart {direction}\n");
    let ids: BTreeMap<&str, String> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.as_str(), format!("n{index}")))
        .collect();

    for node in &graph.nodes {
        let id = &ids[node.id.as_str()];
        let label = mermaid_escape(&node.label);
        output.push_str(&format!("    {id}[\"{label}\"]\n"));
    }

    for edge in &graph.edges {
        if let (Some(source), Some(target)) =
            (ids.get(edge.source.as_str()), ids.get(edge.target.as_str()))
        {
            output.push_str(&format!("    {source} --> {target}\n"));
        }
    }

    output
}

fn is_tag_declaration(line: &str) -> bool {
    let line = line.trim();
    line.starts_with('#')
        && line
            .chars()
            .nth(1)
            .is_some_and(|c| !c.is_whitespace() && c != '#')
}

fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let line = line.trim_start();
    let level = line.chars().take_while(|&c| c == '#').count();

    if !(1..=6).contains(&level) {
        return None;
    }

    let rest = &line[level..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }

    Some((level, rest.trim()))
}

fn first_heading_depth(content: &str) -> usize {
    content
        .lines()
        .filter(|line| !is_tag_declaration(line))
        .find_map(|line| parse_heading(line).map(|(level, _)| level))
        .unwrap_or(7)
}

fn normalize_atom_content(content: &str, parent_level: usize) -> Vec<String> {
    let mut output = Vec::new();
    let mut heading_stack: Vec<(usize, usize)> = Vec::new();

    for line in content.lines().filter(|line| !is_tag_declaration(line)) {
        if let Some((original_level, title)) = parse_heading(line) {
            while heading_stack
                .last()
                .is_some_and(|(level, _)| *level >= original_level)
            {
                heading_stack.pop();
            }

            let local_level = heading_stack
                .last()
                .map_or(1, |(_, normalized)| normalized + 1);

            heading_stack.push((original_level, local_level));

            let level = (parent_level + local_level).min(6).max(1);
            output.push(format!("{} {}", "#".repeat(level), title));
        } else {
            output.push(line.to_string());
        }
    }

    output
}

fn append_markdown(output: &mut Vec<String>, lines: Vec<String>) {
    for line in lines {
        if line.trim().is_empty() {
            if !output.is_empty() && !output.last().unwrap().is_empty() {
                output.push(String::new());
            }
            continue;
        }

        if let Some((_, title)) = parse_heading(&line) {
            let previous_heading = output
                .iter()
                .rev()
                .find(|previous| !previous.trim().is_empty())
                .and_then(|previous| parse_heading(previous))
                .map(|(_, title)| title.to_string());

            if previous_heading.as_deref() == Some(title) {
                continue;
            }
        }

        output.push(line);
    }

    while output.last().is_some_and(|line| line.is_empty()) {
        output.pop();
    }
}

pub fn to_markdown(graph: &Mindmap) -> String {
    let nodes: BTreeMap<&str, &Node> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();

    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut parents = BTreeSet::new();
    let mut memberships: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for edge in &graph.edges {
        match edge.kind {
            EdgeKind::TagHierarchy
                if edge.source.starts_with("tag:")
                    && edge.target.starts_with("tag:") =>
            {
                children
                    .entry(edge.source.clone())
                    .or_default()
                    .push(edge.target.clone());

                parents.insert(edge.target.clone());
            }
            EdgeKind::Membership if edge.source.starts_with("tag:") => {
                memberships
                    .entry(edge.source.clone())
                    .or_default()
                    .push(edge.target.clone());
            }
            _ => {}
        }
    }

    for values in children.values_mut() {
        values.sort();
        values.dedup();
    }

    for values in memberships.values_mut() {
        values.sort();
        values.dedup();
    }

    fn render_tag(
        id: &str,
        level: usize,
        nodes: &BTreeMap<&str, &Node>,
        children: &BTreeMap<String, Vec<String>>,
        memberships: &BTreeMap<String, Vec<String>>,
        output: &mut Vec<String>,
    ) {
        let Some(node) = nodes.get(id) else {
            return;
        };

        let level = level.min(6).max(1);
        append_markdown(
            output,
            vec![format!("{} {}", "#".repeat(level), node.label)],
        );

        // H1 content precedes H2 content, then H3, etc.
        // Atoms without headings are placed after heading-bearing atoms.
        let mut atom_ids = memberships.get(id).cloned().unwrap_or_default();

        atom_ids.sort_by_key(|atom_id| {
            let atom = nodes.get(atom_id.as_str());
            let depth = atom
                .and_then(|node| node.content.as_deref())
                .map(first_heading_depth)
                .unwrap_or(7);

            let label = atom.map(|node| node.label.as_str()).unwrap_or("");
            (depth, label.to_string(), atom_id.clone())
        });

        for atom_id in atom_ids {
            let Some(atom) = nodes.get(atom_id.as_str()) else {
                continue;
            };

            let Some(content) = atom.content.as_deref() else {
                continue;
            };

            append_markdown(output, normalize_atom_content(content, level));
            append_markdown(output, vec![String::new()]);
        }

        if let Some(child_ids) = children.get(id) {
            for child_id in child_ids {
                render_tag(
                    child_id,
                    level + 1,
                    nodes,
                    children,
                    memberships,
                    output,
                );
            }
        }
    }

    let mut output = Vec::new();

    let mut roots: Vec<&Node> = graph
        .nodes
        .iter()
        .filter(|node| node.id.starts_with("tag:"))
        .filter(|node| !parents.contains(&node.id))
        .collect();

    roots.sort_by(|a, b| a.id.cmp(&b.id));

    for root in roots {
        render_tag(
            &root.id,
            1,
            &nodes,
            &children,
            &memberships,
            &mut output,
        );
    }

    // Atoms without regular-tag membership remain in the document.
    // Function tags are deliberately excluded from heading generation.
    let mut uncategorized: Vec<&Node> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Atom)
        .filter(|node| {
            !memberships
                .values()
                .any(|ids| ids.iter().any(|id| id == &node.id))
        })
        .collect();

    uncategorized.sort_by_key(|node| {
        (
            node.content
                .as_deref()
                .map(first_heading_depth)
                .unwrap_or(7),
            node.label.clone(),
            node.id.clone(),
        )
    });

    if !uncategorized.is_empty() {
        for atom in uncategorized {
            if let Some(content) = &atom.content {
                append_markdown(&mut output, normalize_atom_content(content, 0));
                append_markdown(&mut output, vec![String::new()]);
            }
        }
    }

    while output.last().is_some_and(|line| line.is_empty()) {
        output.pop();
    }

    if output.is_empty() {
        String::new()
    } else {
        output.join("\n") + "\n"
    }
}
