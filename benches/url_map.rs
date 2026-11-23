//! Benchmarks for URL map real-time insertion, tree view projection (with and
//! without ID folding), and force-directed domain graph relaxation.
//!
//! Run with `cargo bench --bench url_map`.

use std::hint::black_box;
use std::time::{Duration, SystemTime};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use http::Method;
use http_proxy::url_map::{
    ForceLayout, ObservedInput, STEPS_PER_FRAME, SortDirection, StatusBucket, TreeFilter,
    TreeSortField, UrlMap, build_view_roots, flatten_visible, parse_origin_header, parse_url,
};

const REQUEST_COUNT: usize = 5_000;

const HOSTS: [&str; 8] = [
    "app.example.com",
    "api.example.com",
    "auth.example.com",
    "static.example.com",
    "cdn.tracker.net",
    "metrics.analytics.io",
    "payments.partner.org",
    "media.storage.net",
];
const RESOURCES: [&str; 6] = ["users", "orders", "items", "events", "sessions", "comments"];

fn build_item(i: usize, base_time: SystemTime) -> ObservedInput {
    let methods = [Method::GET, Method::POST, Method::PUT, Method::DELETE];
    let statuses = [
        StatusBucket::S2xx,
        StatusBucket::S2xx,
        StatusBucket::S2xx,
        StatusBucket::S3xx,
        StatusBucket::S4xx,
        StatusBucket::S5xx,
    ];
    let host = HOSTS
        .get(i % HOSTS.len())
        .copied()
        .unwrap_or("app.example.com");
    let resource = RESOURCES
        .get((i / 7) % RESOURCES.len())
        .copied()
        .unwrap_or("items");
    let url_str = match i % 4 {
        0 => format!(
            "https://{host}/api/v1/{resource}/{i}?token=SECRET_{i}&page={}",
            i % 10
        ),
        1 => format!("https://{host}/api/v1/{resource}/{i}/details?expand=true"),
        2 => format!("https://{host}/assets/chunk-{i:04}.js?v={}", i % 5),
        _ => format!("https://{host}/pages/section-{}/view-{}", i % 80, i % 120),
    };
    let Some(target) = parse_url(&url_str) else {
        unreachable!("benchmark URL must parse: {url_str}");
    };
    ObservedInput {
        target,
        method: methods
            .get(i % methods.len())
            .cloned()
            .unwrap_or(Method::GET),
        status: statuses
            .get(i % statuses.len())
            .copied()
            .unwrap_or(StatusBucket::S2xx),
        content_type: Some(if i % 4 == 2 {
            "application/javascript".to_owned()
        } else {
            "application/json".to_owned()
        }),
        bytes: u64::try_from((i % 32) * 512 + 256).unwrap_or(256),
        duration: Some(Duration::from_millis(
            u64::try_from((i % 120) + 5).unwrap_or(5),
        )),
        started_at: base_time + Duration::from_millis(u64::try_from(i).unwrap_or(0)),
        referer: i.is_multiple_of(3).then(|| {
            parse_url("https://app.example.com/dashboard?session=SECRET_REF")
                .unwrap_or_else(|| unreachable!("referer URL must parse"))
        }),
        origin_header: i.is_multiple_of(5).then(|| {
            parse_origin_header("https://app.example.com")
                .unwrap_or_else(|| unreachable!("origin header must parse"))
        }),
        redirect_location: i.is_multiple_of(17).then(|| {
            parse_url("https://auth.example.com/login?next=SECRET_LOC")
                .unwrap_or_else(|| unreachable!("redirect URL must parse"))
        }),
    }
}

fn generate_workload() -> Vec<ObservedInput> {
    let base_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    (0..REQUEST_COUNT)
        .map(|i| build_item(i, base_time))
        .collect()
}

fn build_populated(workload: &[ObservedInput]) -> UrlMap {
    let mut populated = UrlMap::new();
    for item in workload {
        let _ = populated.observe(item.clone());
    }
    assert!(
        populated.node_count() >= 3_000,
        "expected >= 3,000 nodes for benchmark, got {}",
        populated.node_count()
    );
    populated
}

fn insert_bench(criterion: &mut Criterion) {
    let workload = generate_workload();
    let mut group = criterion.benchmark_group("url_map_insert");
    group.throughput(Throughput::Elements(
        u64::try_from(REQUEST_COUNT).unwrap_or(0),
    ));
    group.sample_size(20);
    group.bench_function("insert_5000_requests", |bench| {
        bench.iter(|| {
            let mut map = UrlMap::new();
            for item in &workload {
                let _ = black_box(map.observe(item.clone()));
            }
            map.node_count()
        });
    });
    group.finish();
}

fn view_bench(criterion: &mut Criterion) {
    let workload = generate_workload();
    let populated = build_populated(&workload);
    let mut group = criterion.benchmark_group("url_map_view");
    group.sample_size(20);
    let folded_filter = TreeFilter {
        fold_ids: true,
        ..TreeFilter::default()
    };
    let raw_filter = TreeFilter {
        fold_ids: false,
        ..TreeFilter::default()
    };
    group.bench_function("project_and_flatten_folded", |bench| {
        bench.iter(|| {
            let roots = build_view_roots(
                black_box(&populated),
                &folded_filter,
                TreeSortField::Name,
                SortDirection::Asc,
            );
            flatten_visible(&roots, &|_, depth| depth < 2).len()
        });
    });
    group.bench_function("project_and_flatten_unfolded", |bench| {
        bench.iter(|| {
            let roots = build_view_roots(
                black_box(&populated),
                &raw_filter,
                TreeSortField::Name,
                SortDirection::Asc,
            );
            flatten_visible(&roots, &|_, depth| depth < 2).len()
        });
    });
    group.finish();
}

fn layout_bench(criterion: &mut Criterion) {
    let workload = generate_workload();
    let populated = build_populated(&workload);
    let mut group = criterion.benchmark_group("url_map_graph_layout");
    group.bench_function("sync_and_step_frame", |bench| {
        bench.iter(|| {
            let mut layout = ForceLayout::new(40);
            layout.sync(black_box(populated.graph()));
            let _ = layout.step(STEPS_PER_FRAME);
            layout.nodes().len()
        });
    });
    group.finish();
}

criterion_group!(benches, insert_bench, view_bench, layout_bench);
criterion_main!(benches);
