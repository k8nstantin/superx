//! Tree-sitter-driven extraction — walks a filesystem path, parses
//! every `.rs` file with `tree-sitter-rust`, and produces an
//! in-memory typed graph of nodes (files, functions, classes,
//! modules) + edges (defines, imports). This stage produces NO
//! substrate writes — those are handled by `substrate.rs` after
//! pipeline composition. Keeping extract pure (no I/O to the
//! substrate) makes it cheap to test and to compose with future
//! stages (clustering, analysis) before persistence.

use std::path::{Path, PathBuf};

use tree_sitter::{Node, Parser};
use walkdir::WalkDir;

/// One node detected during extraction. The `index` is its position
/// in [`ExtractedGraph::nodes`] — edges reference nodes by index for
/// the duration of the in-memory build, then [`crate::substrate`]
/// resolves indices to substrate `RecordId`s at persistence time.
#[derive(Debug, Clone)]
pub(crate) struct GmNode {
    pub kind: GmNodeKind,
    pub name: String,
    pub file: PathBuf,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GmNodeKind {
    CodeFile,
    Function,
    Class,
    Module,
}

impl GmNodeKind {
    pub(crate) fn type_uid(self) -> &'static str {
        match self {
            Self::CodeFile => "node_code_file",
            Self::Function => "node_function",
            Self::Class => "node_class",
            Self::Module => "node_module",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct GmEdge {
    pub kind: GmEdgeKind,
    pub from: usize,
    pub to: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum GmEdgeKind {
    Defines,
    Imports,
}

impl GmEdgeKind {
    pub(crate) fn type_uid(self) -> &'static str {
        match self {
            Self::Defines => "edge_defines",
            Self::Imports => "edge_imports",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ExtractedGraph {
    pub nodes: Vec<GmNode>,
    pub edges: Vec<GmEdge>,
}

/// Extract a typed graph from every `.rs` file under `root`. Walks
/// the directory tree, parses each file via `tree-sitter-rust`, and
/// extracts file → function / class / module structure plus
/// `use`-declaration imports.
pub(crate) fn extract_rust(root: &Path) -> Result<ExtractedGraph, ExtractError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::language())
        .map_err(|e| ExtractError::Language(e.to_string()))?;

    let mut graph = ExtractedGraph::default();

    let rust_files: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            p.extension().and_then(|s| s.to_str()) == Some("rs")
                && !p.components().any(|c| c.as_os_str() == "target")
        })
        .collect();

    for file_path in rust_files {
        let source = match std::fs::read_to_string(&file_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let tree = match parser.parse(&source, None) {
            Some(t) => t,
            None => continue,
        };
        extract_file(&mut graph, file_path, source.as_bytes(), tree.root_node());
    }

    Ok(graph)
}

fn extract_file(graph: &mut ExtractedGraph, file_path: PathBuf, source: &[u8], root: Node) {
    // Push the file node first; everything else points back at it via
    // edge_defines / edge_imports.
    let file_name = file_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<unknown>")
        .to_string();
    let file_idx = push_node(
        graph,
        GmNode {
            kind: GmNodeKind::CodeFile,
            name: file_name,
            file: file_path.clone(),
            start_byte: 0,
            end_byte: source.len(),
        },
    );

    walk(graph, file_idx, &file_path, source, root);
}

fn walk(
    graph: &mut ExtractedGraph,
    file_idx: usize,
    file_path: &Path,
    source: &[u8],
    node: Node,
) {
    match node.kind() {
        "function_item" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = bytes_to_string(source, name_node);
                let fn_idx = push_node(
                    graph,
                    GmNode {
                        kind: GmNodeKind::Function,
                        name,
                        file: file_path.to_path_buf(),
                        start_byte: node.start_byte(),
                        end_byte: node.end_byte(),
                    },
                );
                graph.edges.push(GmEdge {
                    kind: GmEdgeKind::Defines,
                    from: file_idx,
                    to: fn_idx,
                });
            }
            // No recursion into function bodies for v1 — we'd otherwise
            // pick up locally-scoped items as if they were top-level.
            return;
        }
        "struct_item" | "enum_item" | "union_item" | "trait_item" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = bytes_to_string(source, name_node);
                let cls_idx = push_node(
                    graph,
                    GmNode {
                        kind: GmNodeKind::Class,
                        name,
                        file: file_path.to_path_buf(),
                        start_byte: node.start_byte(),
                        end_byte: node.end_byte(),
                    },
                );
                graph.edges.push(GmEdge {
                    kind: GmEdgeKind::Defines,
                    from: file_idx,
                    to: cls_idx,
                });
            }
            return;
        }
        "mod_item" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = bytes_to_string(source, name_node);
                let mod_idx = push_node(
                    graph,
                    GmNode {
                        kind: GmNodeKind::Module,
                        name,
                        file: file_path.to_path_buf(),
                        start_byte: node.start_byte(),
                        end_byte: node.end_byte(),
                    },
                );
                graph.edges.push(GmEdge {
                    kind: GmEdgeKind::Defines,
                    from: file_idx,
                    to: mod_idx,
                });
            }
            // Inline `mod foo { ... }` blocks — recurse to capture
            // nested items.
        }
        "use_declaration" => {
            // For v1, model the use-declaration itself as a Module
            // node with the imported path as its name, and link the
            // file to it via edge_imports. Cross-file resolution
            // (i.e. matching a `use crate::foo` to its definition)
            // is a later PR.
            let raw = bytes_to_string(source, node);
            let mod_idx = push_node(
                graph,
                GmNode {
                    kind: GmNodeKind::Module,
                    name: raw,
                    file: file_path.to_path_buf(),
                    start_byte: node.start_byte(),
                    end_byte: node.end_byte(),
                },
            );
            graph.edges.push(GmEdge {
                kind: GmEdgeKind::Imports,
                from: file_idx,
                to: mod_idx,
            });
            return;
        }
        _ => {}
    }

    // Default: recurse into children.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(graph, file_idx, file_path, source, child);
    }
}

fn push_node(graph: &mut ExtractedGraph, n: GmNode) -> usize {
    let idx = graph.nodes.len();
    graph.nodes.push(n);
    idx
}

fn bytes_to_string(source: &[u8], node: Node) -> String {
    std::str::from_utf8(&source[node.start_byte()..node.end_byte()])
        .unwrap_or("<non-utf8>")
        .to_string()
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ExtractError {
    #[error("tree-sitter language load failed: {0}")]
    Language(String),
}
