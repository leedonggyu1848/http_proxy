//! URL Map panel: Tree view (host -> path hierarchy) and Relationship Graph view.

mod detail_pane;
mod endpoint_view;
mod graph_pane;
mod tree_pane;

pub(crate) use detail_pane::RecordMatchCache;

use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::{Layout, Space, StrokeWidth};
use crate::ui::workbench::field::{self, Field};
use crate::ui::workbench::text::{self, ShellText};
use crate::ui::workbench::{chrome, controls};
use std::collections::{HashMap, HashSet};

use eframe::egui;
use http_inspector_analysis::endpoint_schema::EndpointSchemas;
use http_inspector_analysis::filter::RecordList;
use http_inspector_analysis::url_map::{
    ContentTypeFilter, DEFAULT_MAX_GRAPH_DOMAINS, EdgeKind, ForceLayout, PartyFilter, RequestScope,
    SortDirection, StatusFilter, TreeFilter, TreeSortField, UrlMap, ViewNode, ViewNodeKey,
    VisibleRow, build_view_roots, find_view_node, flatten_visible,
};
use http_inspector_exchange::{Exchange, RecordId};

/// Active sub-view inside the Map tab.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum MapSubView {
    /// Host -> path segment tree with inline metrics and right detail pane.
    #[default]
    Tree,
    /// Force-directed domain relationship graph (`Referer`, `Origin`, `Redirect`).
    Graph,
}

/// File export formats supported from the Map panel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MapExportFormat {
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
    pub(super) const fn file_prefix(self) -> &'static str {
        match self {
            Self::TreeText | Self::TreeJson => "http-inspector-url-map",
            Self::GraphMermaid | Self::GraphDot => "http-inspector-url-graph",
        }
    }

    pub(super) const fn extension(self) -> &'static str {
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
pub(super) enum MapAction {
    /// Filter the Requests view to a node's subtree or one endpoint and switch to it.
    ShowInRequests(RequestScope),
    /// Select a specific retained exchange and switch to the Requests view.
    SelectRequest(RecordId),
    /// Save an exported representation into the configured `Save directory`.
    Export(MapExportFormat),
    /// Copy an exported representation to the clipboard.
    Copy(MapExportFormat),
    /// Clear captured records and the URL map.
    ClearAll,
}

/// Edge visibility flags for the relationship graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct EdgeVisibility {
    pub(super) referer: bool,
    pub(super) origin: bool,
    pub(super) redirect: bool,
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
    pub(super) const fn allows(self, kind: EdgeKind) -> bool {
        match kind {
            EdgeKind::Referer => self.referer,
            EdgeKind::Origin => self.origin,
            EdgeKind::Redirect => self.redirect,
        }
    }
}

/// UI state and cached view projection for the URL Map tab.
pub(super) struct MapPanel {
    pub(super) sub_view: MapSubView,
    pub(super) filter: TreeFilter,
    pub(super) sort: TreeSortField,
    pub(super) direction: SortDirection,
    pub(super) selected: Option<ViewNodeKey>,
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
    pub(super) fn new() -> Self {
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
    pub(super) fn clear(&mut self) {
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
    pub(super) const fn max_graph_domains(&self) -> usize {
        self.layout.max_domains()
    }

    /// Reports whether ID folding is currently active.
    pub(super) const fn fold_ids(&self) -> bool {
        self.filter.fold_ids
    }

    /// Rebuilds the cached projected tree and flattened rows only when inputs changed.
    pub(super) fn ensure_projection(&mut self, map: &UrlMap) {
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

    /// The pane header of the tree pane, or of the whole graph: the Tree
    /// and Graph tabs on the left and the pane's actions on the right (D22).
    pub(super) fn header(
        &mut self,
        ui: &mut egui::Ui,
        map: &UrlMap,
        save_busy: bool,
    ) -> Option<MapAction> {
        chrome::pane_header(ui, "map-header", |ui| {
            self.sub_view_tabs(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                controls::ghost_style(ui);
                let clear = ui
                    .add(controls::ghost(
                        ShellText::Clear.text(),
                        Layout::PANE_BUTTON_HEIGHT,
                    ))
                    .on_hover_text(ShellText::ClearHint.text())
                    .clicked()
                    .then_some(MapAction::ClearAll);
                let action = Self::export_menu(ui, map, save_busy).or(clear);
                if self.sub_view == MapSubView::Graph {
                    self.graph_actions(ui);
                }
                action
            })
            .inner
        })
    }

    /// The host filter, filter menus, sort, and expansion of the tree, in
    /// one pane row under the header. Wraps when the pane is narrow.
    pub(super) fn tree_filters(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .inner_margin(Layout::PANE_ROW_MARGIN)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    controls::ghost_style(ui);
                    ui.spacing_mut().item_spacing = egui::vec2(Space::XS, Space::XS);
                    self.tree_controls(ui);
                });
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.cursor().top() - StrokeWidth::STRUCTURAL / 2.0,
            egui::Stroke::new(StrokeWidth::STRUCTURAL, Palette::of(ui).borders.subtle),
        );
    }

    /// Renders the left tree pane (used when `sub_view == MapSubView::Tree`).
    pub(super) fn show_tree_pane(
        &mut self,
        ui: &mut egui::Ui,
        map: &UrlMap,
        shortcuts_allowed: bool,
    ) {
        self.ensure_projection(map);
        tree_pane::show(self, ui, map, shortcuts_allowed);
    }

    /// Renders the right detail pane for the selected tree node.
    pub(super) fn show_detail_pane(
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
    pub(super) fn show_graph_pane(&mut self, ui: &mut egui::Ui, map: &UrlMap) {
        let selected_host = self
            .selected
            .as_ref()
            .map(|key| key.origin.host().to_owned());
        if let Some(host) = graph_pane::show(
            ui,
            map.graph(),
            &mut self.layout,
            self.edge_visibility,
            &mut self.dragged_node,
            selected_host.as_deref(),
        ) {
            self.jump_to_host(map, &host);
        }
    }

    /// Domains and edges the graph shows, and whether the domain limit hid
    /// some, for the status bar.
    pub(super) fn graph_summary(&self) -> (String, bool) {
        graph_pane::summary_text(&self.layout, self.edge_visibility)
    }

    /// Jumps from the relationship graph to the first origin matching `host` in Tree view.
    ///
    /// Keeps active tree filters when a matching origin is already visible in the
    /// projected tree. If `host` exists in `map` but is hidden by the active tree
    /// filters, first clears only `host_query` to preserve matching status,
    /// content-type, and party filters; if still hidden, clears the remaining
    /// blocking filters. If `host` is absent from `map`, leaves active filters
    /// unchanged.
    pub(super) fn jump_to_host(&mut self, map: &UrlMap, host: &str) {
        self.sub_view = MapSubView::Tree;
        self.ensure_projection(map);

        let mut target_key = self.first_root_for_host(host);
        if target_key.is_none() && map.origins().keys().any(|origin| origin.host() == host) {
            if !self.filter.host_query.is_empty() {
                self.filter.host_query.clear();
                self.ensure_projection(map);
                target_key = self.first_root_for_host(host);
            }

            if target_key.is_none() {
                self.filter.status = StatusFilter::All;
                self.filter.content_type = ContentTypeFilter::All;
                self.filter.party = PartyFilter::All;
                self.ensure_projection(map);
                target_key = self.first_root_for_host(host);
            }
        }

        if let Some(key) = target_key {
            let _ = self.expanded_overrides.insert(key.clone(), true);
            self.selected = Some(key.clone());
            self.rows_dirty = true;
            self.ensure_projection(map);
            self.keyboard_scroll_to = self.cached_rows.iter().position(|row| row.key == key);
        }
    }

    fn first_root_for_host(&self, host: &str) -> Option<ViewNodeKey> {
        self.cached_roots
            .iter()
            .find(|root| root.key.origin.host() == host)
            .map(|root| root.key.clone())
    }

    pub(super) fn set_expanded(&mut self, key: ViewNodeKey, expanded: bool) {
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

    fn sub_view_tabs(&mut self, ui: &mut egui::Ui) {
        controls::tab_list(ui, ShellText::MapView.text(), |ui| {
            for (view, label) in [
                (MapSubView::Tree, ShellText::Tree),
                (MapSubView::Graph, ShellText::Graph),
            ] {
                if controls::text_tab(ui, self.sub_view == view, label.text()).clicked() {
                    self.sub_view = view;
                }
            }
        });
    }

    fn tree_controls(&mut self, ui: &mut egui::Ui) {
        let width = Layout::MAP_SEARCH_WIDTH.fit(ui.available_width());
        let field = Field::new(width)
            .hint(ShellText::FilterHosts.text())
            .search_icon()
            .monospace();
        let _ = field::single_line(ui, &mut self.filter.host_query, field, |_| {});
        choice_menu(
            ui,
            &mut self.filter.status,
            &StatusFilter::ALL,
            StatusFilter::label,
        );
        choice_menu(
            ui,
            &mut self.filter.content_type,
            &ContentTypeFilter::ALL,
            ContentTypeFilter::label,
        );
        choice_menu(
            ui,
            &mut self.filter.party,
            &PartyFilter::ALL,
            PartyFilter::label,
        );
        let fold = controls::ghost_toggle(
            ui,
            self.filter.fold_ids,
            ShellText::FoldIds.text(),
            Layout::PANE_BUTTON_HEIGHT,
        )
        .on_hover_text(ShellText::FoldIdsHint.text());
        if fold.clicked() {
            self.filter.fold_ids = !self.filter.fold_ids;
        }
        let sort = text::sort_label(self.sort.label());
        let _ = controls::ghost_menu(
            ui,
            text::menu_label(&sort),
            Layout::PANE_BUTTON_HEIGHT,
            |ui| {
                for option in TreeSortField::ALL {
                    ui.selectable_value(&mut self.sort, option, option.label());
                }
            },
        );
        let direction = match self.direction {
            SortDirection::Asc => ShellText::Ascending,
            SortDirection::Desc => ShellText::Descending,
        };
        if ui
            .add(controls::ghost(
                self.direction.arrow(),
                Layout::PANE_BUTTON_HEIGHT,
            ))
            .on_hover_text(direction.text())
            .clicked()
        {
            self.direction = self.direction.toggled();
        }
        for (label, expanded) in [
            (ShellText::ExpandAll, true),
            (ShellText::CollapseAll, false),
        ] {
            if ui
                .add(controls::ghost(label.text(), Layout::PANE_BUTTON_HEIGHT))
                .clicked()
            {
                self.set_all_expanded(expanded);
            }
        }
    }

    /// Edge kinds with their line samples, the domain limit, and Relayout,
    /// laid out right to left in the graph's pane header.
    fn graph_actions(&mut self, ui: &mut egui::Ui) {
        if ui
            .add(controls::ghost(
                ShellText::Relayout.text(),
                Layout::PANE_BUTTON_HEIGHT,
            ))
            .on_hover_text(ShellText::RelayoutHint.text())
            .clicked()
        {
            self.layout = ForceLayout::new(self.layout.max_domains());
        }
        let mut limit = self.layout.max_domains();
        let _ = controls::ghost_menu(
            ui,
            text::menu_label(&text::top_domains(limit)),
            Layout::PANE_BUTTON_HEIGHT,
            |ui| {
                for count in [15_usize, 40, 80, 150] {
                    ui.selectable_value(&mut limit, count, text::top_domains(count));
                }
            },
        );
        self.layout.set_max_domains(limit);
        let _ = controls::ghost_menu(
            ui,
            text::menu_label(ShellText::Legend.text()),
            Layout::PANE_BUTTON_HEIGHT,
            |ui| graph_pane::legend_menu(ui, &mut self.edge_visibility),
        );
    }

    fn export_menu(ui: &mut egui::Ui, map: &UrlMap, save_busy: bool) -> Option<MapAction> {
        let has_data = map.node_count() > 0;
        ui.add_enabled_ui(!save_busy && has_data, |ui| {
            controls::ghost_menu(
                ui,
                text::menu_label(ShellText::Export.text()),
                Layout::PANE_BUTTON_HEIGHT,
                |ui| {
                    let mut action = None;
                    for (label, chosen) in [
                        (
                            ShellText::SaveTreeText,
                            MapAction::Export(MapExportFormat::TreeText),
                        ),
                        (
                            ShellText::SaveMapJson,
                            MapAction::Export(MapExportFormat::TreeJson),
                        ),
                        (
                            ShellText::SaveGraphMermaid,
                            MapAction::Export(MapExportFormat::GraphMermaid),
                        ),
                        (
                            ShellText::SaveGraphDot,
                            MapAction::Export(MapExportFormat::GraphDot),
                        ),
                        (
                            ShellText::CopyMermaid,
                            MapAction::Copy(MapExportFormat::GraphMermaid),
                        ),
                        (
                            ShellText::CopyDot,
                            MapAction::Copy(MapExportFormat::GraphDot),
                        ),
                    ] {
                        if matches!(label, ShellText::SaveGraphMermaid | ShellText::CopyMermaid) {
                            ui.separator();
                        }
                        if ui.button(label.text()).clicked() {
                            action = Some(chosen);
                            ui.close();
                        }
                    }
                    action
                },
            )
            .inner
            .flatten()
        })
        .inner
    }
}

/// A Ghost menu that shows the current choice of `value` and lists the
/// others.
fn choice_menu<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    value: &mut T,
    options: &[T],
    label: impl Fn(T) -> &'static str,
) {
    let _ = controls::ghost_menu(
        ui,
        text::menu_label(label(*value)),
        Layout::PANE_BUTTON_HEIGHT,
        |ui| {
            for &option in options {
                ui.selectable_value(value, option, label(option));
            }
        },
    );
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
    use http_inspector_analysis::url::parse_url;
    use http_inspector_analysis::url_map::{ObservedInput, StatusBucket};
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

    fn observe_url(map: &mut UrlMap, url: &str, status: StatusBucket, content_type: Option<&str>) {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let Some(target) = parse_url(url) else {
            unreachable!("valid url: {url}");
        };
        let _ = map.observe(ObservedInput {
            target,
            method: Method::GET,
            status,
            content_type: content_type.map(str::to_owned),
            bytes: 128,
            duration: None,
            started_at: now,
            referer: None,
            origin_header: None,
            redirect_location: None,
        });
    }

    #[test]
    fn jump_to_host_preserves_port_and_scheme_filters_matching_origin() {
        let mut map = UrlMap::new();
        observe_url(
            &mut map,
            "http://example.test/plain",
            StatusBucket::S2xx,
            Some("application/json"),
        );
        observe_url(
            &mut map,
            "https://example.test:8443/secure",
            StatusBucket::S2xx,
            Some("application/json"),
        );

        for query in [":8443", "https://"] {
            let mut panel = MapPanel::new();
            panel.sub_view = MapSubView::Graph;
            panel.filter.host_query = query.to_owned();
            panel.filter.status = StatusFilter::S2xx;
            panel.filter.content_type = ContentTypeFilter::Json;
            panel.filter.party = PartyFilter::FirstParty;

            panel.jump_to_host(&map, "example.test");

            assert_eq!(panel.sub_view, MapSubView::Tree);
            assert_eq!(panel.filter.host_query, query);
            assert_eq!(panel.filter.status, StatusFilter::S2xx);
            assert_eq!(panel.filter.content_type, ContentTypeFilter::Json);
            assert_eq!(panel.filter.party, PartyFilter::FirstParty);
            assert_eq!(panel.cached_roots.len(), 1);
            assert_eq!(
                panel
                    .cached_roots
                    .first()
                    .map(|root| root.key.display_url())
                    .as_deref(),
                Some("https://example.test:8443")
            );
            let Some(selected) = panel.selected.as_ref() else {
                unreachable!("matching root should be selected");
            };
            assert_eq!(selected.display_url(), "https://example.test:8443");
            assert_eq!(panel.expanded_overrides.get(selected), Some(&true));
            assert_eq!(panel.keyboard_scroll_to, Some(0));
        }
    }

    #[test]
    fn jump_to_host_clears_non_matching_host_filter_when_host_exists() {
        let mut map = UrlMap::new();
        observe_url(
            &mut map,
            "https://example.test:8443/secure",
            StatusBucket::S2xx,
            Some("text/html"),
        );
        observe_url(
            &mut map,
            "https://other.test/page",
            StatusBucket::S2xx,
            Some("text/html"),
        );

        let mut panel = MapPanel::new();
        panel.sub_view = MapSubView::Graph;
        panel.filter.host_query = "other.test".to_owned();

        panel.jump_to_host(&map, "example.test");

        assert_eq!(panel.sub_view, MapSubView::Tree);
        assert_eq!(panel.filter.host_query, "");
        assert_eq!(
            panel
                .selected
                .as_ref()
                .map(ViewNodeKey::display_url)
                .as_deref(),
            Some("https://example.test:8443")
        );
    }

    #[test]
    fn jump_to_host_clears_only_host_filter_when_other_filters_match() {
        let mut map = UrlMap::new();
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let Some(first_party) = parse_url("http://localhost:18080/app") else {
            unreachable!("valid url");
        };
        let Some(target_3rd) = parse_url("http://127.0.0.1:18443/secure") else {
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
            target: target_3rd,
            method: Method::GET,
            status: StatusBucket::S2xx,
            content_type: Some("application/json".to_owned()),
            bytes: 120,
            duration: None,
            started_at: now,
            referer: Some(first_party),
            origin_header: None,
            redirect_location: None,
        });

        let mut panel = MapPanel::new();
        panel.sub_view = MapSubView::Graph;
        panel.filter.host_query = "localhost".to_owned();
        panel.filter.status = StatusFilter::S2xx;
        panel.filter.content_type = ContentTypeFilter::Json;
        panel.filter.party = PartyFilter::ThirdParty;

        panel.jump_to_host(&map, "127.0.0.1");

        assert_eq!(panel.sub_view, MapSubView::Tree);
        assert_eq!(panel.filter.host_query, "");
        assert_eq!(panel.filter.status, StatusFilter::S2xx);
        assert_eq!(panel.filter.content_type, ContentTypeFilter::Json);
        assert_eq!(panel.filter.party, PartyFilter::ThirdParty);
        let Some(selected) = panel.selected.as_ref() else {
            unreachable!("matching root should be selected");
        };
        assert_eq!(selected.display_url(), "http://127.0.0.1:18443");
        assert_eq!(panel.expanded_overrides.get(selected), Some(&true));
        assert_eq!(panel.keyboard_scroll_to, Some(0));
    }

    #[test]
    fn jump_to_host_keeps_active_filters_when_host_is_absent_from_map() {
        let mut map = UrlMap::new();
        observe_url(
            &mut map,
            "https://example.test:8443/secure",
            StatusBucket::S2xx,
            Some("text/html"),
        );

        let mut panel = MapPanel::new();
        panel.sub_view = MapSubView::Graph;
        panel.filter.host_query = ":8443".to_owned();
        panel.filter.status = StatusFilter::S2xx;
        panel.filter.content_type = ContentTypeFilter::Html;
        panel.filter.party = PartyFilter::FirstParty;

        panel.jump_to_host(&map, "missing.test");

        assert_eq!(panel.sub_view, MapSubView::Tree);
        assert_eq!(panel.filter.host_query, ":8443");
        assert_eq!(panel.filter.status, StatusFilter::S2xx);
        assert_eq!(panel.filter.content_type, ContentTypeFilter::Html);
        assert_eq!(panel.filter.party, PartyFilter::FirstParty);
        assert!(panel.selected.is_none());
    }
}
