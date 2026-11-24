//! Builds the [`TreeModel`] of an XML preview, with an `XPath` for every
//! element. Document parsing applies the node and depth limits.

use super::pretty::push_leaf;
use super::{Document, Element, NodeKind, XmlFormatError};
use crate::format::tag::{Dialect, Spacing, push_tag};
use crate::format::tree::{CopyLabels, TreeModel, TreeNode, elide};
use crate::format::{Styled, TokenKind};

const COPY: CopyLabels = CopyLabels {
    path: "Copy XPath",
    value: "Copy value (XML)",
};

/// Builds the rows of `text` as far as it could be read. Whitespace-only
/// text is left out; fails when `text` has no element.
pub(in crate::format) fn build(text: &str) -> Result<TreeModel, XmlFormatError> {
    let document = Document::parse(text)?;
    let mut builder = Builder {
        document: &document,
        nodes: Vec::new(),
    };
    builder.children(&document.roots, None, "", 0);
    Ok(TreeModel::new(
        builder.nodes,
        document.incomplete_notice(),
        COPY,
    ))
}

struct Builder<'d, 'a> {
    document: &'d Document<'a>,
    nodes: Vec<TreeNode>,
}

impl Builder<'_, '_> {
    /// Adds the rows of `indexes` under the row `parent`, whose element is
    /// at `parent_path`, and returns how many rows were added.
    fn children(
        &mut self,
        indexes: &[usize],
        parent: Option<usize>,
        parent_path: &str,
        depth: usize,
    ) -> usize {
        let document = self.document;
        let names = indexes
            .iter()
            .map(|&index| element_name(document, index))
            .collect::<Vec<_>>();
        let mut count = 0;
        for (position, &index) in indexes.iter().enumerate() {
            let Some(node) = document.node(index) else {
                continue;
            };
            let raw = document.slice(&node.range);
            match (&node.kind, names.get(position).copied().flatten()) {
                (NodeKind::Element(element), Some(name)) => {
                    let path = xpath(parent_path, name, &names, position);
                    self.element(index, element, path, parent, depth);
                }
                (NodeKind::Text, _) if raw.trim().is_empty() => continue,
                (kind, _) => {
                    let mut label = Styled::default();
                    push_leaf(&mut label, kind, &row_text(raw.trim()));
                    self.push(label, None, raw.to_owned(), parent, depth);
                }
            }
            count += 1;
        }
        count
    }

    fn element(
        &mut self,
        index: usize,
        element: &Element,
        path: String,
        parent: Option<usize>,
        depth: usize,
    ) {
        let document = self.document;
        let start_tag = document
            .node(index)
            .map_or("", |node| document.slice(&node.range));
        let value = document
            .text
            .get(start(document, index)..end(document, index))
            .unwrap_or_default()
            .to_owned();
        let mut label = Styled::default();
        push_tag(
            &mut label,
            &row_text(start_tag),
            Dialect::Xml,
            Spacing::Normalize,
        );
        if let Some(text) = self.only_text(element) {
            label.push(&row_text(text.trim()), TokenKind::Text);
            if let Some(end_tag) = &element.end_tag {
                push_tag(
                    &mut label,
                    document.slice(end_tag),
                    Dialect::Xml,
                    Spacing::Normalize,
                );
            }
            self.push(label, Some(path), value, parent, depth);
            return;
        }
        let row = self.push(label, Some(path.clone()), value, parent, depth);
        let count = self.children(&element.children, Some(row), &path, depth + 1);
        if count > 0
            && let Some(node) = self.nodes.get_mut(row)
        {
            let noun = if count == 1 { "child" } else { "children" };
            node.summary = Some(format!("… {count} {noun}"));
        }
    }

    /// The text of an element whose only content is one text node.
    fn only_text(&self, element: &Element) -> Option<&str> {
        let [child] = element.children.as_slice() else {
            return None;
        };
        let node = self.document.node(*child)?;
        (node.kind == NodeKind::Text).then(|| self.document.slice(&node.range))
    }

    fn push(
        &mut self,
        label: Styled,
        path: Option<String>,
        value: String,
        parent: Option<usize>,
        depth: usize,
    ) -> usize {
        self.nodes.push(TreeNode {
            name: label.text().to_owned(),
            label,
            path,
            value,
            summary: None,
            parent,
            depth,
        });
        self.nodes.len() - 1
    }
}

fn start(document: &Document<'_>, index: usize) -> usize {
    document.node(index).map_or(0, |node| node.range.start)
}

/// Where the source of the node at `index` ends: after the closing tag of
/// an element, or after its last descendant when it was cut off.
fn end(document: &Document<'_>, index: usize) -> usize {
    let Some(node) = document.node(index) else {
        return 0;
    };
    match &node.kind {
        NodeKind::Element(Element {
            end_tag: Some(end_tag),
            ..
        }) => end_tag.end,
        NodeKind::Element(element) => element.children.last().map_or(node.range.end, |&last| {
            end(document, last).max(node.range.end)
        }),
        _ => node.range.end,
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

/// Collapses runs of whitespace so a row stays on one line, then elides it.
fn row_text(text: &str) -> String {
    elide(&text.split_whitespace().collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::tree::ROW_TEXT;

    fn rows(tree: &TreeModel, parent: Option<usize>) -> Vec<(String, Option<String>, usize)> {
        tree.children(parent)
            .into_iter()
            .filter_map(|index| {
                let node = tree.node(index)?;
                Some((
                    node.label.text().to_owned(),
                    node.path.clone(),
                    tree.child_count(index),
                ))
            })
            .collect()
    }

    #[test]
    fn builds_rows_with_paths_and_child_counts() -> Result<(), Box<dyn std::error::Error>> {
        let tree = build(concat!(
            "<?xml version=\"1.0\"?>\n<rss version=\"2.0\"><channel><title>News</title>",
            "<item><title>A</title></item><!-- x --><item><title>B</title><n:x/></item>",
            "</channel></rss>"
        ))?;
        assert_eq!(tree.incomplete, None);
        assert_eq!(
            rows(&tree, None),
            [
                ("<?xml version=\"1.0\"?>".to_owned(), None, 0),
                (
                    "<rss version=\"2.0\">".to_owned(),
                    Some("/rss".to_owned()),
                    1
                ),
            ]
        );
        let rss = tree.children(None).get(1).copied();
        let channel = rss.and_then(|rss| tree.children(Some(rss)).first().copied());
        let Some(channel_node) = channel.and_then(|index| tree.node(index)) else {
            return Err("missing channel".into());
        };
        assert_eq!(channel_node.depth, 1);
        assert_eq!(channel_node.summary.as_deref(), Some("… 4 children"));
        assert_eq!(
            rows(&tree, channel),
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
        let second = channel.and_then(|channel| tree.children(Some(channel)).get(3).copied());
        let paths = second.map(|item| {
            rows(&tree, Some(item))
                .into_iter()
                .map(|(_, path, _)| path)
                .collect::<Vec<_>>()
        });
        let value = second
            .and_then(|item| tree.node(item))
            .map(|item| item.value.as_str());
        assert_eq!(value, Some("<item><title>B</title><n:x/></item>"));
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
    fn keeps_partial_trees_and_elides_long_rows() -> Result<(), XmlFormatError> {
        let long = "word ".repeat(100);
        let text = format!("<a><b>{long}</b><c>\n  mixed <d/></c><e");
        let tree = build(&text)?;
        assert!(tree.incomplete.is_some());
        let a = tree.children(None).first().copied();
        let children = a.map_or_else(Vec::new, |a| rows(&tree, Some(a)));
        let labels = children
            .iter()
            .map(|(label, _, count)| (label.chars().count(), *count))
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            [(ROW_TEXT + "<b>".len() + 1 + "</b>".len(), 0), (3, 2)]
        );
        assert_eq!(
            a.and_then(|a| tree.node(a)).map(|a| a.value.as_str()),
            text.strip_suffix("<e"),
            "a cut-off element's value runs to its last descendant"
        );
        assert!(build("not xml").is_err());
        Ok(())
    }
}
