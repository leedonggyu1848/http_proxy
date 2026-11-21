//! Collapsible element tree of an XML preview, with an `XPath` for every
//! element.

use super::pretty::{push_leaf, push_tag};
use super::{Document, Element, NodeKind};
use crate::ui::body_format::{Styled, TokenKind};

/// Longest text shown on one tree row; the rest is elided.
const ROW_TEXT: usize = 200;

/// One row of the tree.
#[derive(Debug, Eq, PartialEq)]
pub(in crate::ui) struct TreeNode {
    /// Highlighted row text: the opening tag, or a whole short element such
    /// as `<title>News</title>`, or a text, comment, or `CDATA` node.
    pub(in crate::ui) label: Styled,
    /// `XPath` of an element, such as `/rss/channel/item[2]`.
    pub(in crate::ui) path: Option<String>,
    pub(in crate::ui) children: Vec<usize>,
    /// Element nesting level, starting at 0 for the root element.
    pub(in crate::ui) depth: usize,
}

/// The rows of an XML preview read as far as it could be.
#[derive(Debug, Eq, PartialEq)]
pub(in crate::ui) struct XmlTree {
    pub(in crate::ui) nodes: Vec<TreeNode>,
    pub(in crate::ui) roots: Vec<usize>,
    pub(in crate::ui) incomplete: Option<String>,
}

impl XmlTree {
    /// Builds the tree of `text`. Whitespace-only text is left out; fails
    /// when `text` has no element.
    pub(in crate::ui) fn build(text: &str) -> Result<Self, String> {
        let document = Document::parse(text)?;
        let mut builder = Builder {
            document: &document,
            nodes: Vec::new(),
        };
        let roots = builder.children(&document.roots, "", 0);
        Ok(Self {
            nodes: builder.nodes,
            roots,
            incomplete: document.incomplete_notice(),
        })
    }

    pub(in crate::ui) fn node(&self, index: usize) -> Option<&TreeNode> {
        self.nodes.get(index)
    }
}

struct Builder<'d, 'a> {
    document: &'d Document<'a>,
    nodes: Vec<TreeNode>,
}

impl Builder<'_, '_> {
    /// Adds the rows of `indexes` under the element at `parent_path`.
    fn children(&mut self, indexes: &[usize], parent_path: &str, depth: usize) -> Vec<usize> {
        let document = self.document;
        let names = indexes
            .iter()
            .map(|&index| element_name(document, index))
            .collect::<Vec<_>>();
        let mut rows = Vec::new();
        for (position, &index) in indexes.iter().enumerate() {
            let Some(node) = document.node(index) else {
                continue;
            };
            let raw = document.slice(&node.range);
            let row = match (&node.kind, names.get(position).copied().flatten()) {
                (NodeKind::Element(element), Some(name)) => {
                    let path = xpath(parent_path, name, &names, position);
                    self.element(raw, element, path, depth)
                }
                (NodeKind::Text, _) if raw.trim().is_empty() => continue,
                (kind, _) => {
                    let mut label = Styled::default();
                    push_leaf(&mut label, kind, &elide(raw.trim()));
                    self.push(TreeNode {
                        label,
                        path: None,
                        children: Vec::new(),
                        depth,
                    })
                }
            };
            rows.push(row);
        }
        rows
    }

    fn element(&mut self, start_tag: &str, element: &Element, path: String, depth: usize) -> usize {
        let mut label = Styled::default();
        push_tag(&mut label, &elide(start_tag), true);
        if let Some(text) = self.only_text(element) {
            label.push(&elide(text.trim()), TokenKind::Text);
            if let Some(end_tag) = &element.end_tag {
                push_tag(&mut label, self.document.slice(end_tag), true);
            }
            return self.push(TreeNode {
                label,
                path: Some(path),
                children: Vec::new(),
                depth,
            });
        }
        let children = self.children(&element.children, &path, depth + 1);
        self.push(TreeNode {
            label,
            path: Some(path),
            children,
            depth,
        })
    }

    /// The text of an element whose only content is one text node.
    fn only_text(&self, element: &Element) -> Option<&str> {
        let [child] = element.children.as_slice() else {
            return None;
        };
        let node = self.document.node(*child)?;
        (node.kind == NodeKind::Text).then(|| self.document.slice(&node.range))
    }

    fn push(&mut self, node: TreeNode) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }
}

fn element_name<'a>(document: &Document<'a>, index: usize) -> Option<&'a str> {
    match &document.node(index)?.kind {
        NodeKind::Element(element) => Some(document.slice(&element.name)),
        _ => None,
    }
}

/// `XPath` of the element `name` at `position` among its siblings' `names`,
/// with a 1-based index when siblings share the name.
fn xpath(parent: &str, name: &str, names: &[Option<&str>], position: usize) -> String {
    let same = |other: Option<&str>| other == Some(name);
    let total = names.iter().filter(|other| same(**other)).count();
    if total > 1 {
        let index = names
            .iter()
            .take(position + 1)
            .filter(|other| same(**other))
            .count();
        format!("{parent}/{name}[{index}]")
    } else {
        format!("{parent}/{name}")
    }
}

/// Collapses runs of whitespace and cuts text longer than [`ROW_TEXT`].
fn elide(text: &str) -> String {
    let mut words = text.split_whitespace();
    let mut out = words.next().unwrap_or_default().to_owned();
    for word in words {
        out.push(' ');
        out.push_str(word);
    }
    if let Some((cut, _)) = out.char_indices().nth(ROW_TEXT) {
        out.truncate(cut);
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(tree: &XmlTree, indexes: &[usize]) -> Vec<(String, Option<String>, usize)> {
        indexes
            .iter()
            .filter_map(|&index| tree.node(index))
            .map(|node| {
                (
                    node.label.text().to_owned(),
                    node.path.clone(),
                    node.children.len(),
                )
            })
            .collect()
    }

    #[test]
    fn builds_rows_with_paths_and_child_counts() -> Result<(), String> {
        let tree = XmlTree::build(concat!(
            "<?xml version=\"1.0\"?>\n<rss version=\"2.0\"><channel><title>News</title>",
            "<item><title>A</title></item><!-- x --><item><title>B</title><n:x/></item>",
            "</channel></rss>"
        ))?;
        assert_eq!(tree.incomplete, None);
        assert_eq!(
            rows(&tree, &tree.roots),
            [
                ("<?xml version=\"1.0\"?>".to_owned(), None, 0),
                (
                    "<rss version=\"2.0\">".to_owned(),
                    Some("/rss".to_owned()),
                    1
                ),
            ]
        );
        let rss = tree.roots.get(1).and_then(|&index| tree.node(index));
        let channel = rss
            .and_then(|rss| rss.children.first())
            .and_then(|&index| tree.node(index));
        let Some(channel) = channel else {
            return Err("missing channel".to_owned());
        };
        assert_eq!(channel.depth, 1);
        assert_eq!(
            rows(&tree, &channel.children),
            [
                (
                    "<title>News</title>".to_owned(),
                    Some("/rss/channel/title".to_owned()),
                    0
                ),
                (
                    "<item>".to_owned(),
                    Some("/rss/channel/item[1]".to_owned()),
                    1
                ),
                ("<!-- x -->".to_owned(), None, 0),
                (
                    "<item>".to_owned(),
                    Some("/rss/channel/item[2]".to_owned()),
                    2
                ),
            ]
        );
        let second = channel.children.get(3).and_then(|&index| tree.node(index));
        let paths = second.map(|item| {
            rows(&tree, &item.children)
                .into_iter()
                .map(|(_, path, _)| path)
                .collect::<Vec<_>>()
        });
        assert_eq!(
            paths,
            Some(vec![
                Some("/rss/channel/item[2]/title".to_owned()),
                Some("/rss/channel/item[2]/n:x".to_owned()),
            ])
        );
        Ok(())
    }

    #[test]
    fn keeps_partial_trees_and_elides_long_rows() -> Result<(), String> {
        let long = "word ".repeat(100);
        let tree = XmlTree::build(&format!("<a><b>{long}</b><c>\n  mixed <d/></c><e"))?;
        assert!(tree.incomplete.is_some());
        let a = tree.roots.first().and_then(|&index| tree.node(index));
        let children = a.map_or_else(Vec::new, |a| rows(&tree, &a.children));
        let labels = children
            .iter()
            .map(|(label, _, count)| (label.chars().count(), *count))
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            [(ROW_TEXT + "<b>".len() + 1 + "</b>".len(), 0), (3, 2)]
        );
        assert!(XmlTree::build("not xml").is_err());
        Ok(())
    }
}
