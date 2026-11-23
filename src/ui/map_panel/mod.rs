//! URL Map panel: Tree view (host -> path hierarchy) and Relationship Graph view.

mod detail_pane;
mod endpoint_view;
mod graph_pane;
mod tree_pane;

pub(in crate::ui) use detail_pane::RecordMatchCache;

use std::collections::{HashMap, HashSet};

use eframe::egui;
use http_proxy::endpoint_schema::EndpointSchemas;
use http_proxy::url_map::{
    ContentTypeFilter, DEFAULT_MAX_GRAPH_DOMAINS, EdgeKind, ForceLayout, PartyFilter, RequestScope,
    SortDirection, StatusFilter, TreeFilter, TreeSortField, UrlMap, ViewNode, ViewNodeKey,
    VisibleRow, build_view_roots, find_view_node, flatten_visible,
};
use http_proxy::{Exchange, RecordId};

use crate::ui::records::RecordList;

/// Active sub-view inside the Map tab.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::ui) enum MapSubView {
    /// Host -> path segment tree with inline metrics and right detail pane.
    #[default]
    Tree,
    /// Force-directed domain relationship graph (`Referer`, `Origin`, `Redirect`).
    Graph,
}

/// File export formats supported from the Map panel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum MapExportFormat {
    /// Indented plain-text tree.
    TreeText,
    /// Structured JSON (nodes, aggregates, query names, domain & URL edges).
    TreeJson,
    /// Mermaid flowchart of the domain relationship graph.
    GraphMermaid,
    /// Graphviz DOT of the domain relationship graph.
    GraphDot,
}

impl MapExportFormat {
    pub(in crate::ui) const fn file_prefix(self) -> &'static str {
        match self {
            Self::TreeText | Self::TreeJson => "http-inspector-url-map",
            Self::GraphMermaid | Self::GraphDot => "http-inspector-url-graph",
        }
    }

    pub(in crate::ui) const fn extension(self) -> &'static str {
        match self {
            Self::TreeText => "txt",
            Self::TreeJson => "json",
            Self::GraphMermaid => "mmd",
            Self::GraphDot => "dot",
        }
    }
}

/// Actions emitted by [`MapPanel`] for [`crate::ui::InspectorApp`] to handle.
#[derive(Clone, Debug)]
pub(in crate::ui) enum MapAction {
    /// Filter the Requests view to a node's subtree or one endpoint and switch to it.
    ShowInRequests(RequestScope),
    /// Select a specific retained exchange and switch to the Requests view.
    SelectRequest(RecordId),
    /// Save an exported representation into the configured `Save directory`.
    Export(MapExportFormat),
    /// Clear captured records and the URL map.
    ClearAll,
}

/// Edge visibility flags for the relationship graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) struct EdgeVisibility {
    pub(in crate::ui) referer: bool,
    pub(in crate::ui) origin: bool,
    pub(in crate::ui) redirect: bool,
}

impl Default for EdgeVisibility {
    fn default() -> Self {
        Self {
            referer: true,
            origin: true,
            redirect: true,
        }
    }
}

impl EdgeVisibility {
    pub(in crate::ui) const fn allows(self, kind: EdgeKind) -> bool {
        match kind {
            EdgeKind::Referer => self.referer,
            EdgeKind::Origin => self.origin,
            EdgeKind::Redirect => self.redirect,
        }
    }
}

/// UI state and cached view projection for the URL Map tab.
pub(in crate::ui) struct MapPanel {
    pub(in crate::ui) sub_view: MapSubView,
    pub(in crate::ui) filter: TreeFilter,
    pub(in crate::ui) sort: TreeSortField,
    pub(in crate::ui) direction: SortDirection,
    pub(in crate::ui) selected: Option<ViewNodeKey>,
    expanded_overrides: HashMap<ViewNodeKey, bool>,
    cached_roots: Vec<ViewNode>,
    cached_rows: Vec<VisibleRow>,
    cached_version: u64,
    cached_filter: TreeFilter,
    cached_sort: TreeSortField,
    cached_direction: SortDirection,
    rows_dirty: bool,
    keyboard_scroll_to: Option<usize>,
    page_rows: usize,
    layout: ForceLayout,
    edge_visibility: EdgeVisibility,
    dragged_node: Option<usize>,
    detail_match_cache: RecordMatchCache,
    endpoint_cache: endpoint_view::EndpointCache,
}

impl Default for MapPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl MapPanel {
    pub(in crate::ui) fn new() -> Self {
        Self {
            sub_view: MapSubView::Tree,
            filter: TreeFilter::default(),
            sort: TreeSortField::Name,
            direction: SortDirection::Asc,
            selected: None,
            expanded_overrides: HashMap::new(),
            cached_roots: Vec::new(),
            cached_rows: Vec::new(),
            cached_version: u64::MAX,
            cached_filter: TreeFilter::default(),
            cached_sort: TreeSortField::Name,
            cached_direction: SortDirection::Asc,
            rows_dirty: true,
            keyboard_scroll_to: None,
            page_rows: 10,
            layout: ForceLayout::new(DEFAULT_MAX_GRAPH_DOMAINS),
            edge_visibility: EdgeVisibility::default(),
            dragged_node: None,
            detail_match_cache: RecordMatchCache::default(),
            endpoint_cache: endpoint_view::EndpointCache::default(),
        }
    }

    /// Resets transient selection, caches, and graph state after `Clear`.
    pub(in crate::ui) fn clear(&mut self) {
        self.selected = None;
        self.expanded_overrides.clear();
        self.cached_roots.clear();
        self.cached_rows.clear();
        self.cached_version = u64::MAX;
        self.rows_dirty = true;
        self.detail_match_cache.clear();
        self.endpoint_cache.clear();
        self.keyboard_scroll_to = None;
        self.dragged_node = None;
        self.layout = ForceLayout::new(self.layout.max_domains());
    }

    /// Returns the currently configured top-N domain limit for graph exports.
    pub(in crate::ui) const fn max_graph_domains(&self) -> usize {
        self.layout.max_domains()
    }

    /// Reports whether ID folding is currently active.
    pub(in crate::ui) const fn fold_ids(&self) -> bool {
        self.filter.fold_ids
    }

    /// Rebuilds the cached projected tree and flattened rows only when inputs changed.
    pub(in crate::ui) fn ensure_projection(&mut self, map: &UrlMap) {
        let needs_rebuild = self.cached_version != map.version()
            || self.cached_filter != self.filter
            || self.cached_sort != self.sort
            || self.cached_direction != self.direction;

        if needs_rebuild {
            self.cached_roots = build_view_roots(map, &self.filter, self.sort, self.direction);
            self.cached_version = map.version();
            self.cached_filter = self.filter.clone();
            self.cached_sort = self.sort;
            self.cached_direction = self.direction;
            self.rows_dirty = true;
        }

        if self.rows_dirty {
            let overrides = &self.expanded_overrides;
            self.cached_rows = flatten_visible(&self.cached_roots, &|key, depth| {
                overrides.get(key).copied().unwrap_or(depth == 0)
            });
            self.rows_dirty = false;
        }
    }

    /// Renders the controls bar above the Map main area.
    pub(in crate::ui) fn controls(
        &mut self,
        ui: &mut egui::Ui,
        map: &UrlMap,
        save_busy: bool,
    ) -> Option<MapAction> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            self.sub_view_toggle(ui);
            ui.separator();
            match self.sub_view {
                MapSubView::Tree => self.tree_controls(ui),
                MapSubView::Graph => self.graph_controls(ui),
            }
            ui.separator();
            if let Some(export_action) = self.export_menu(ui, map, save_busy) {
                action = Some(export_action);
            }
            if ui
                .button("Clear")
                .on_hover_text("Clear captured requests and reset the URL map.")
                .clicked()
            {
                action = Some(MapAction::ClearAll);
            }
        });
        action
    }

    /// Renders the left tree pane (used when `sub_view == MapSubView::Tree`).
    pub(in crate::ui) fn show_tree_pane(
        &mut self,
        ui: &mut egui::Ui,
        map: &UrlMap,
        shortcuts_allowed: bool,
    ) {
        self.ensure_projection(map);
        tree_pane::show(self, ui, map, shortcuts_allowed);
    }

    /// Renders the right detail pane for the selected tree node.
    pub(in crate::ui) fn show_detail_pane(
        &mut self,
        ui: &mut egui::Ui,
        map: &UrlMap,
        records: &RecordList<Exchange>,
        schemas: &EndpointSchemas,
    ) -> Option<MapAction> {
        self.ensure_projection(map);
        let selected_node = self
            .selected
            .as_ref()
            .and_then(|key| find_view_node(&self.cached_roots, key));
        let endpoints = detail_pane::Endpoints {
            merged: selected_node.map_or(&[], |node| self.endpoint_cache.sync(node, schemas)),
            skipped: schemas.skipped_requests(),
        };
        detail_pane::show(
            ui,
            selected_node,
            records,
            &mut self.detail_match_cache,
            &endpoints,
        )
    }

    /// Renders the interactive domain relationship graph.
    pub(in crate::ui) fn show_graph_pane(&mut self, ui: &mut egui::Ui, map: &UrlMap) {
        if let Some(host) = graph_pane::show(
            ui,
            map.graph(),
            &mut self.layout,
            self.edge_visibility,
            &mut self.dragged_node,
        ) {
            self.jump_to_host(map, &host);
        }
    }

    /// Jumps from the relationship graph to the first origin matching `host` in Tree view.
    ///
    /// If `host` exists in `map` but is hidden by the active tree filters, clears
    /// the blocking filters so the host is visible, expanded, and selected.
    pub(in crate::ui) fn jump_to_host(&mut self, map: &UrlMap, host: &str) {
        self.sub_view = MapSubView::Tree;
        if !self.filter.host_query.is_empty()
            && !host
                .to_ascii_lowercase()
                .contains(&self.filter.host_query.trim().to_ascii_lowercase())
        {
            self.filter.host_query.clear();
        }
        self.ensure_projection(map);

        let exists_in_map = map.origins().keys().any(|origin| origin.host() == host);
        let mut target_key = self
            .cached_roots
            .iter()
            .find(|root| root.key.origin.host() == host)
            .map(|root| root.key.clone());

        if target_key.is_none() && exists_in_map {
            self.filter.host_query.clear();
            self.filter.status = StatusFilter::All;
            self.filter.content_type = ContentTypeFilter::All;
            self.filter.party = PartyFilter::All;
            self.ensure_projection(map);
            target_key = self
                .cached_roots
                .iter()
                .find(|root| root.key.origin.host() == host)
                .map(|root| root.key.clone());
        }

        if let Some(key) = target_key {
            let _ = self.expanded_overrides.insert(key.clone(), true);
            self.selected = Some(key.clone());
            self.rows_dirty = true;
            self.ensure_projection(map);
            self.keyboard_scroll_to = self.cached_rows.iter().position(|row| row.key == key);
        }
    }

    pub(in crate::ui) fn set_expanded(&mut self, key: ViewNodeKey, expanded: bool) {
        let _ = self.expanded_overrides.insert(key, expanded);
        self.rows_dirty = true;
    }

    fn set_all_expanded(&mut self, expanded: bool) {
        let mut keys = HashSet::new();
        collect_branch_keys(&self.cached_roots, &mut keys);
        for key in keys {
            let _ = self.expanded_overrides.insert(key, expanded);
        }
        self.rows_dirty = true;
    }

    fn sub_view_toggle(&mut self, ui: &mut egui::Ui) {
        ui.selectable_value(&mut self.sub_view, MapSubView::Tree, "Tree");
        ui.selectable_value(&mut self.sub_view, MapSubView::Graph, "Relationships");
    }

    fn tree_controls(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width().clamp(90.0, 150.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.filter.host_query)
                .desired_width(width)
                .hint_text("Filter host…"),
        );
        egui::ComboBox::from_id_salt("map-status-filter")
            .selected_text(self.filter.status.label())
            .show_ui(ui, |ui| {
                for option in StatusFilter::ALL {
                    ui.selectable_value(&mut self.filter.status, option, option.label());
                }
            });
        egui::ComboBox::from_id_salt("map-type-filter")
            .selected_text(self.filter.content_type.label())
            .show_ui(ui, |ui| {
                for option in ContentTypeFilter::ALL {
                    ui.selectable_value(&mut self.filter.content_type, option, option.label());
                }
            });
        egui::ComboBox::from_id_salt("map-party-filter")
            .selected_text(self.filter.party.label())
            .show_ui(ui, |ui| {
                for option in PartyFilter::ALL {
                    ui.selectable_value(&mut self.filter.party, option, option.label());
                }
            });
        ui.checkbox(&mut self.filter.fold_ids, "Fold IDs")
            .on_hover_text("Group sibling ID segments (integers, UUIDs, hex tokens) into /{id}.");
        egui::ComboBox::from_id_salt("map-sort-field")
            .selected_text(format!("Sort: {}", self.sort.label()))
            .show_ui(ui, |ui| {
                for option in TreeSortField::ALL {
                    ui.selectable_value(&mut self.sort, option, option.label());
                }
            });
        if ui
            .button(self.direction.arrow())
            .on_hover_text("Toggle ascending / descending sort order.")
            .clicked()
        {
            self.direction = self.direction.toggled();
        }
        if ui.small_button("Expand all").clicked() {
            self.set_all_expanded(true);
        }
        if ui.small_button("Collapse all").clicked() {
            self.set_all_expanded(false);
        }
    }

    fn graph_controls(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.edge_visibility.referer, "Referer");
        ui.checkbox(&mut self.edge_visibility.origin, "Origin (CORS)");
        ui.checkbox(&mut self.edge_visibility.redirect, "Redirect (3xx)");
        let mut limit = self.layout.max_domains();
        egui::ComboBox::from_id_salt("map-graph-limit")
            .selected_text(format!("Top {limit} domains"))
            .show_ui(ui, |ui| {
                for count in [15_usize, 40, 80, 150] {
                    ui.selectable_value(&mut limit, count, format!("Top {count} domains"));
                }
            });
        self.layout.set_max_domains(limit);
        if ui
            .button("Relayout")
            .on_hover_text("Recompute the force-directed domain layout.")
            .clicked()
        {
            self.layout = ForceLayout::new(self.layout.max_domains());
        }
    }

    fn export_menu(&self, ui: &mut egui::Ui, map: &UrlMap, save_busy: bool) -> Option<MapAction> {
        let mut action = None;
        let has_data = map.node_count() > 0;
        ui.add_enabled_ui(!save_busy && has_data, |ui| {
            ui.menu_button("Export ▼", |ui| {
                ui.weak("Files save into Save directory without overwriting.");
                if ui.button("Save tree as Text (.txt)").clicked() {
                    action = Some(MapAction::Export(MapExportFormat::TreeText));
                    ui.close();
                }
                if ui.button("Save map as JSON (.json)").clicked() {
                    action = Some(MapAction::Export(MapExportFormat::TreeJson));
                    ui.close();
                }
                ui.separator();
                if ui.button("Save graph as Mermaid (.mmd)").clicked() {
                    action = Some(MapAction::Export(MapExportFormat::GraphMermaid));
                    ui.close();
                }
                if ui.button("Save graph as Graphviz DOT (.dot)").clicked() {
                    action = Some(MapAction::Export(MapExportFormat::GraphDot));
                    ui.close();
                }
                ui.separator();
                if ui.button("Copy Mermaid to clipboard").clicked() {
                    let mermaid = http_proxy::interop::map_export::export_graph_mermaid(
                        map,
                        self.layout.max_domains(),
                    );
                    ui.ctx().copy_text(mermaid);
                    ui.close();
                }
                if ui.button("Copy DOT to clipboard").clicked() {
                    let dot = http_proxy::interop::map_export::export_graph_dot(
                        map,
                        self.layout.max_domains(),
                    );
                    ui.ctx().copy_text(dot);
                    ui.close();
                }
            });
        });
        action
    }
}

fn collect_branch_keys(nodes: &[ViewNode], out: &mut HashSet<ViewNodeKey>) {
    for node in nodes {
        if !node.children.is_empty() {
            let _ = out.insert(node.key.clone());
            collect_branch_keys(&node.children, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;
    use http_proxy::url_map::{ObservedInput, StatusBucket, parse_url};
    use std::time::{Duration, SystemTime};

    #[test]
    fn jump_to_host_clears_blocking_filters_so_clicked_domain_is_selected() {
        let mut map = UrlMap::new();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let Some(first_party) = parse_url("https://app.example.test/home") else {
            unreachable!("valid url");
        };
        let Some(third_party) = parse_url("https://cdn.other.net/app.js") else {
            unreachable!("valid url");
        };
        let _ = map.observe(ObservedInput {
            target: first_party.clone(),
            method: Method::GET,
            status: StatusBucket::S2xx,
            content_type: Some("text/html".to_owned()),
            bytes: 100,
            duration: None,
            started_at: now,
            referer: None,
            origin_header: None,
            redirect_location: None,
        });
        let _ = map.observe(ObservedInput {
            target: third_party,
            method: Method::GET,
            status: StatusBucket::S5xx,
            content_type: Some("application/javascript".to_owned()),
            bytes: 200,
            duration: None,
            started_at: now,
            referer: Some(first_party),
            origin_header: None,
            redirect_location: None,
        });

        let mut panel = MapPanel::new();
        panel.sub_view = MapSubView::Graph;
        panel.filter.status = StatusFilter::S5xx;
        panel.filter.content_type = ContentTypeFilter::Script;
        panel.filter.party = PartyFilter::ThirdParty;

        panel.jump_to_host(&map, "app.example.test");
        assert_eq!(panel.sub_view, MapSubView::Tree);
        assert_eq!(panel.filter.status, StatusFilter::All);
        assert_eq!(panel.filter.content_type, ContentTypeFilter::All);
        assert_eq!(panel.filter.party, PartyFilter::All);
        assert_eq!(
            panel.selected.as_ref().map(|k| k.origin.host()),
            Some("app.example.test")
        );
    }
}
