//! Micro-benchmarks for the hot paths that back the "performant (incremental)"
//! goal:
//!
//! * `analyze` — full parse + summary extraction (runs on every changed doc).
//! * `folding` / `diagnostics` — per-request feature computation off a cached
//!   analysis.
//! * `to_reference_links` — the formatter transform.
//! * `incremental_edit` — applying a single content change to a large rope,
//!   which is what `didChange` does under INCREMENTAL sync.
//!
//! Run with: `cargo bench` (or `nix develop --command cargo bench`).

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};

use ropey::Rope;
use sanemark::analysis::analyze;
use sanemark::config::{CompletionConfig, DiagnosticsConfig, FoldingConfig};
use sanemark::document::Document;
use sanemark::encoding::PositionEncoding;
use sanemark::features::formatting::to_reference_links;
use sanemark::features::tables::format_tables;
use sanemark::features::{completion::complete, diagnostics::diagnostics, folding::folding_ranges};
use sanemark::fuzzy::PathMatcher;
use tower_lsp_server::ls_types::{Position, Range, TextDocumentContentChangeEvent};

/// Build a realistic Markdown document with `sections` sections, each with a
/// heading, a paragraph containing two inline links, a bulleted list and a
/// fenced code block.
fn make_document(sections: usize) -> String {
    let mut s = String::with_capacity(sections * 200);
    s.push_str("---\ntitle: Benchmark\ntags: [a, b]\n---\n\n");
    for i in 0..sections {
        s.push_str(&format!("# Section {i}\n\n"));
        s.push_str(&format!(
            "Intro paragraph with [an inline link](https://example.com/{i}) and \
             [a local link](./notes/file{i}.md) to explore.\n\n"
        ));
        s.push_str("Some notes:\n\n");
        for j in 0..5 {
            s.push_str(&format!("- item {i}.{j} with `code`\n"));
        }
        s.push('\n');
        s.push_str(&format!(
            "```rust\nfn f{i}() -> u32 {{\n    {i}\n}}\n```\n\n"
        ));
        s.push_str("> A blockquote to round things out.\n\n");
    }
    s
}

fn bench_analyze(c: &mut Criterion) {
    let doc = make_document(200);
    c.bench_function("analyze/200_sections", |b| {
        b.iter(|| analyze(black_box(&doc), 1))
    });
}

fn bench_folding(c: &mut Criterion) {
    let doc = make_document(200);
    let analysis = analyze(&doc, 1);
    let rope = Rope::from_str(&doc);
    let config = FoldingConfig::default();
    c.bench_function("folding/200_sections", |b| {
        b.iter(|| folding_ranges(black_box(&analysis), black_box(&rope), &config))
    });
}

fn bench_diagnostics(c: &mut Criterion) {
    // No filesystem base -> external links are skipped and relative links resolve
    // against a nonexistent dir, so this measures pure link enumeration + range
    // computation without disk I/O jitter.
    let doc = make_document(200);
    let analysis = analyze(&doc, 1);
    let rope = Rope::from_str(&doc);
    let config = DiagnosticsConfig::default();
    c.bench_function("diagnostics/200_sections", |b| {
        b.iter(|| {
            diagnostics(
                black_box(&analysis),
                black_box(&rope),
                &config,
                PositionEncoding::Utf16,
                None,
                None,
            )
        })
    });
}

fn bench_formatting(c: &mut Criterion) {
    let doc = make_document(200);
    c.bench_function("to_reference_links/200_sections", |b| {
        b.iter(|| to_reference_links(black_box(&doc), true, true, "References"))
    });
}

fn bench_incremental_edit(c: &mut Criterion) {
    let doc = make_document(200);
    let base = Document::new(&doc, 1);
    // Insert a character near the middle of the document.
    let mid_line = (base.rope.len_lines() / 2) as u32;
    let change = TextDocumentContentChangeEvent {
        range: Some(Range::new(
            Position::new(mid_line, 0),
            Position::new(mid_line, 0),
        )),
        range_length: None,
        text: "x".to_string(),
    };
    c.bench_function("incremental_edit/200_sections", |b| {
        b.iter_batched(
            || base.clone(),
            |mut d| d.apply_change(black_box(&change), PositionEncoding::Utf16),
            BatchSize::SmallInput,
        )
    });
}

fn bench_table_formatting(c: &mut Criterion) {
    // A large-ish table: 200 rows x 6 columns, unaligned and pipe-ragged.
    let mut doc = String::from("col a | col b | col c | col d | col e | col f\n");
    for i in 0..200 {
        doc.push_str(&format!(
            "value {i} | {i} | some text | {i}.0 | tag-{i} | note {i}\n"
        ));
    }
    c.bench_function("format_tables/200_rows", |b| {
        b.iter(|| format_tables(black_box(&doc)))
    });
}

fn bench_fuzzy(c: &mut Criterion) {
    // Score a query against a realistic set of nested path candidates.
    let candidates: Vec<String> = (0..2000)
        .map(|i| format!("src/module{}/submod/file{}.md", i % 40, i))
        .collect();
    c.bench_function("fuzzy/score_2000_paths", |b| {
        b.iter(|| {
            let mut matcher = PathMatcher::new("mod file md");
            let mut hits = 0u32;
            for cand in &candidates {
                if matcher.score(black_box(cand)).is_some() {
                    hits += 1;
                }
            }
            hits
        })
    });
}

/// Build a temp tree of `dirs` directories each holding `per_dir` markdown files.
fn make_tree(dirs: usize, per_dir: usize) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for d in 0..dirs {
        let sub = root.path().join(format!("dir{d}")).join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        for f in 0..per_dir {
            std::fs::write(sub.join(format!("file{d}_{f}.md")), "").unwrap();
        }
    }
    root
}

fn bench_completion_deep(c: &mut Criterion) {
    // ~50 * 20 = 1000 nested files across two directory levels.
    let tree = make_tree(50, 20);
    let doc_path = tree.path().join("index.md");
    let config = CompletionConfig::default();

    // Empty query -> the walk must scan (and cap) the whole tree.
    let rope_all = Rope::from_str("[x](./");
    c.bench_function("completion/deep_empty_query_1000_files", |b| {
        b.iter(|| {
            complete(
                black_box(&rope_all),
                Position::new(0, 6),
                PositionEncoding::Utf16,
                &config,
                Some(&doc_path),
                Some(tree.path()),
            )
        })
    });

    // Targeted fuzzy query -> most candidates are filtered out.
    let rope_q = Rope::from_str("[x](./file3_5");
    c.bench_function("completion/deep_fuzzy_query_1000_files", |b| {
        b.iter(|| {
            complete(
                black_box(&rope_q),
                Position::new(0, 13),
                PositionEncoding::Utf16,
                &config,
                Some(&doc_path),
                Some(tree.path()),
            )
        })
    });
}

criterion_group!(
    benches,
    bench_analyze,
    bench_folding,
    bench_diagnostics,
    bench_formatting,
    bench_table_formatting,
    bench_fuzzy,
    bench_completion_deep,
    bench_incremental_edit
);
criterion_main!(benches);
