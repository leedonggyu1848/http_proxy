//! Format-independent rows of a body Tree view. Each format only turns text
//! into a [`TreeModel`]; [`crate::ui::tree_view`] draws any model the same
//! way.

use super::{BodyFormat, Styled};

/// Longest text shown on one tree row; the rest is elided.
pub(in crate::ui) const ROW_TEXT: usize = 200;

/// One row of a tree.
#[derive(Debug, Eq, PartialEq)]
pub(in crate::ui) struct TreeNode {
    /// Highlighted row text, such as `"id": 42` or `<item>`.
    pub(in crate::ui) label: Styled,
    /// The part of the row that search matches, such as a JSON key or an XML
    /// row; leaf rows also match their [`TreeNode::value`].
    pub(in crate::ui) name: String,
    /// Path copied from the row, such as `$.items[0]` or `/rss/channel`.
    pub(in crate::ui) path: Option<String>,
    /// Source text copied from the row and shown when hovering it.
    pub(in crate::ui) value: String,
    /// Shown after the label while the row is collapsed.
    pub(in crate::ui) summary: Option<String>,
    pub(in crate::ui) parent: Option<usize>,
    /// Nesting level, starting at 0 for top-level rows.
    pub(in crate::ui) depth: usize,
}

/// Context menu wording of one format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) struct CopyLabels {
    pub(in crate::ui) path: &'static str,
    pub(in crate::ui) value: &'static str,
}

/// Rows in depth-first order: every row comes after its parent and before
/// its next sibling, and children are one level deeper than their parent.
#[derive(Debug, Eq, PartialEq)]
pub(in crate::ui) struct TreeModel {
    nodes: Vec<TreeNode>,
    child_counts: Vec<usize>,
    /// Why reading stopped before the end of the text, if it did.
    pub(in crate::ui) incomplete: Option<String>,
    pub(in crate::ui) copy: CopyLabels,
}

impl TreeModel {
    /// `nodes` must be in depth-first order with parents before children.
    pub(in crate::ui) fn new(
        nodes: Vec<TreeNode>,
        incomplete: Option<String>,
        copy: CopyLabels,
    ) -> Self {
        let mut child_counts = vec![0; nodes.len()];
        for parent in nodes.iter().filter_map(|node| node.parent) {
            if let Some(count) = child_counts.get_mut(parent) {
                *count += 1;
            }
        }
        Self {
            nodes,
            child_counts,
            incomplete,
            copy,
        }
    }

    pub(in crate::ui) fn nodes(&self) -> &[TreeNode] {
        &self.nodes
    }

    pub(in crate::ui) fn node(&self, index: usize) -> Option<&TreeNode> {
        self.nodes.get(index)
    }

    pub(in crate::ui) fn child_count(&self, index: usize) -> usize {
        self.child_counts.get(index).copied().unwrap_or_default()
    }

    #[cfg(test)]
    /// Indexes of the rows directly under `parent`, or the top-level rows.
    pub(in crate::ui) fn children(&self, parent: Option<usize>) -> Vec<usize> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.parent == parent)
            .map(|(index, _)| index)
            .collect()
    }
}

/// Builds the tree of `text` for `format`; `None` when the format has no
/// tree. The builders apply their format's node and depth limits.
pub(in crate::ui) fn build(format: BodyFormat, text: &str) -> Option<Result<TreeModel, String>> {
    format.spec().tree.map(|build| build(text))
}

/// Cuts `text` longer than [`ROW_TEXT`] characters and marks the cut.
pub(in crate::ui) fn elide(text: &str) -> String {
    match text.char_indices().nth(ROW_TEXT) {
        Some((cut, _)) => format!("{}…", text.get(..cut).unwrap_or_default()),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elides_long_text_by_characters() {
        assert_eq!(elide("short"), "short");
        let long = "é".repeat(ROW_TEXT + 5);
        let cut = elide(&long);
        assert_eq!(cut.chars().count(), ROW_TEXT + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(elide(&"a".repeat(ROW_TEXT)), "a".repeat(ROW_TEXT));
    }

    #[test]
    fn only_json_and_xml_have_trees() {
        assert!(build(BodyFormat::Json, "[1]").is_some_and(|tree| tree.is_ok()));
        assert!(build(BodyFormat::Xml, "<a/>").is_some_and(|tree| tree.is_ok()));
        assert!(build(BodyFormat::Css, "a{}").is_none());
    }
}
