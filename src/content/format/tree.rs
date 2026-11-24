//! Format-independent rows of a body Tree view. Each format only turns text
//! into a [`TreeModel`]; the UI tree view draws any model the same
//! way.

use super::{BodyFormat, FormatError, Styled};

/// Longest text shown on one tree row; the rest is elided.
pub(in crate::content::format) const ROW_TEXT: usize = 200;

/// One row of a tree.
#[derive(Debug, Eq, PartialEq)]
pub struct TreeNode {
    /// Highlighted row text, such as `"id": 42` or `<item>`.
    pub label: Styled,
    /// The part of the row that search matches, such as a JSON key or an XML
    /// row; leaf rows also match their [`TreeNode::value`].
    pub name: String,
    /// Path copied from the row, such as `$.items[0]` or `/rss/channel`.
    pub path: Option<String>,
    /// Source text copied from the row and shown when hovering it.
    pub value: String,
    /// Shown after the label while the row is collapsed.
    pub summary: Option<String>,
    pub parent: Option<usize>,
    /// Nesting level, starting at 0 for top-level rows.
    pub depth: usize,
}

/// Context menu wording of one format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyLabels {
    pub path: &'static str,
    pub value: &'static str,
}

/// Rows in depth-first order: every row comes after its parent and before
/// its next sibling, and children are one level deeper than their parent.
#[derive(Debug, Eq, PartialEq)]
pub struct TreeModel {
    nodes: Vec<TreeNode>,
    child_counts: Vec<usize>,
    /// Why reading stopped before the end of the text, if it did.
    pub incomplete: Option<String>,
    pub copy: CopyLabels,
}

impl TreeModel {
    /// `nodes` must be in depth-first order with parents before children.
    #[must_use]
    pub(in crate::content::format) fn new(
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

    #[must_use]
    pub fn nodes(&self) -> &[TreeNode] {
        &self.nodes
    }

    #[must_use]
    pub fn node(&self, index: usize) -> Option<&TreeNode> {
        self.nodes.get(index)
    }

    #[must_use]
    pub fn child_count(&self, index: usize) -> usize {
        self.child_counts.get(index).copied().unwrap_or_default()
    }

    #[cfg(test)]
    /// Indexes of the rows directly under `parent`, or the top-level rows.
    #[must_use]
    pub fn children(&self, parent: Option<usize>) -> Vec<usize> {
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
#[must_use]
pub fn build(format: BodyFormat, text: &str) -> Option<Result<TreeModel, FormatError>> {
    format.spec().tree.map(|build| build(text))
}

/// Cuts `text` longer than [`ROW_TEXT`] characters and marks the cut.
#[must_use]
pub(in crate::content::format) fn elide(text: &str) -> String {
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
