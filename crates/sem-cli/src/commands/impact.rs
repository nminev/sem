use std::{collections::HashSet, path::Path};

use colored::Colorize;
use sem_core::git::bridge::GitBridge;
use sem_core::parser::graph::{EntityGraph, EntityInfo};
use sem_mcp::cache::CacheSourceScope;

use crate::cache::{CachedImpactMode, DiskCache};
use crate::impact_model::{
    ImpactQueryError, ImpactReport, ImpactSource, ResolvedImpact, TestEvidence,
};
use crate::timings::Timings;

pub struct ImpactOptions {
    pub cwd: String,
    pub entity_name: Option<String>,
    pub entity_id: Option<String>,
    pub file_hint: Option<String>,
    pub json: bool,
    pub file_exts: Vec<String>,
    pub mode: ImpactMode,
    pub depth: usize,
    pub no_cache: bool,
    pub no_default_excludes: bool,
}

#[derive(Clone, Copy)]
pub enum ImpactMode {
    All,
    Deps,
    Dependents,
    Tests,
}

const LARGE_IMPACT_CACHE_MISS_FILE_THRESHOLD: usize = 20_000;

/// Run a graph build behind a uv-style spinner, then clear it and print a
/// summary before the result is used (so the spinner never interleaves with
/// command output). `count` extracts the entity count for the summary line.
fn build_with_spinner<T>(
    file_count: usize,
    build: impl FnOnce() -> T,
    count: impl FnOnce(&T) -> usize,
) -> T {
    let prog = crate::progress::Progress::start_staged();
    let result = build();
    prog.done(&format!(
        "{} entities, {} files",
        super::graph::fmt_count(count(&result)),
        super::graph::fmt_count(file_count)
    ));
    result
}

pub fn impact_command(opts: ImpactOptions) {
    let started = std::time::Instant::now();
    let mut timings = Timings::from_env("impact");

    let resolved = match resolve_impact(&opts, &mut timings) {
        Ok(resolved) => resolved,
        Err(error) => print_impact_error(error),
    };
    let source = resolved.source;
    render_and_finish_impact(&resolved, &opts, timings);
    if source == ImpactSource::Local {
        super::consent::maybe_cloud_tip(&opts.cwd, started.elapsed());
    }
}

fn resolve_impact(
    opts: &ImpactOptions,
    timings: &mut Timings,
) -> Result<ResolvedImpact, ImpactQueryError> {
    // A resident server beats the cloud path on both freshness and latency,
    // so the sidecar goes first.
    if let Some(report) = try_sidecar_impact(opts) {
        timings.mark("sidecar_impact_query");
        return Ok(ResolvedImpact::new(report, ImpactSource::Sidecar));
    }

    if let Some(report) = super::cloud::try_cloud_impact(opts) {
        timings.mark("cloud_impact_query");
        return Ok(ResolvedImpact::new(report, ImpactSource::Cloud));
    }

    let root = match GitBridge::open(Path::new(&opts.cwd)) {
        Ok(git) => git.repo_root().to_path_buf(),
        Err(_) => Path::new(&opts.cwd).to_path_buf(),
    };
    let root = root.as_path();
    let registry = super::create_registry(&root.to_string_lossy());

    let ext_filter = super::graph::normalize_exts(&opts.file_exts);
    let source_scope =
        super::graph::cache_source_scope(root, &ext_filter, opts.no_default_excludes);
    let file_hint = opts
        .file_hint
        .as_deref()
        .map(|file| super::normalize_repo_relative_path(Path::new(&opts.cwd), root, file));
    let cache_first_entity_scope = opts.entity_id.is_some() || file_hint.is_some();

    if !opts.no_cache
        && matches!(opts.mode, ImpactMode::Deps)
        && matches!(source_scope, CacheSourceScope::Default)
        && cache_first_entity_scope
    {
        match DiskCache::open(root) {
            Ok(disk) => {
                timings.mark("cache_open");
                match try_cached_impact_query(
                    &disk,
                    root,
                    &[],
                    opts,
                    file_hint.as_deref(),
                    source_scope,
                    true,
                    timings,
                ) {
                    Ok(Some(report)) => {
                        return Ok(ResolvedImpact::new(report, ImpactSource::DiskCache));
                    }
                    Ok(None) => {}
                    Err(error) => return Err(error),
                }
            }
            Err(_) => {
                timings.mark("cache_open_failed");
            }
        }
    }

    let file_paths = super::graph::find_supported_files_with_options(
        root,
        &registry,
        &ext_filter,
        opts.no_default_excludes,
    );
    timings.mark("file_discovery");

    if !opts.no_cache {
        match DiskCache::open(root) {
            Ok(disk) => {
                timings.mark("cache_open");
                match try_cached_impact_query(
                    &disk,
                    root,
                    &file_paths,
                    opts,
                    file_hint.as_deref(),
                    source_scope,
                    false,
                    timings,
                ) {
                    Ok(Some(report)) => {
                        return Ok(ResolvedImpact::new(report, ImpactSource::DiskCache));
                    }
                    Ok(None) => {}
                    Err(error) => return Err(error),
                }
            }
            Err(_) => {
                timings.mark("cache_open_failed");
            }
        }
    }

    let report = match opts.mode {
        ImpactMode::Deps => {
            let graph = build_with_spinner(
                file_paths.len(),
                || {
                    if opts.no_cache || file_paths.len() > LARGE_IMPACT_CACHE_MISS_FILE_THRESHOLD {
                        let entity_name = opts.entity_name.clone();
                        let entity_id = opts.entity_id.clone();
                        let file_hint_for_match = file_hint.clone();
                        super::graph::get_or_build_direct_dependency_graph_with_timings(
                            root,
                            &file_paths,
                            &registry,
                            opts.no_cache,
                            source_scope,
                            timings,
                            move |entity| {
                                if let Some(id) = entity_id.as_deref() {
                                    return entity.id == id;
                                }
                                let Some(name) = entity_name.as_deref() else {
                                    return false;
                                };
                                if file_hint_for_match
                                    .as_deref()
                                    .is_some_and(|file| entity.file_path != file)
                                {
                                    return false;
                                }
                                super::entity_matches_query(entity, name)
                            },
                        )
                    } else {
                        super::graph::get_or_build_graph_topology_with_timings(
                            root,
                            &file_paths,
                            &registry,
                            opts.no_cache,
                            source_scope,
                            timings,
                        )
                    }
                },
                |g| g.entities.len(),
            );
            let entity = find_entity(
                &graph,
                opts.entity_name.as_deref(),
                opts.entity_id.as_deref(),
                file_hint.as_deref(),
            )?;
            timings.mark("entity_lookup");
            let mut report = ImpactReport::for_entity(entity.clone());
            report.dependencies = graph
                .get_dependencies(&entity.id)
                .into_iter()
                .cloned()
                .collect();
            report
        }
        ImpactMode::Dependents => {
            let graph = build_with_spinner(
                file_paths.len(),
                || {
                    if file_paths.len() > LARGE_IMPACT_CACHE_MISS_FILE_THRESHOLD {
                        super::graph::get_or_build_graph_topology_with_topology_save_on_miss_with_timings(
                            root,
                            &file_paths,
                            &registry,
                            opts.no_cache,
                            source_scope,
                            timings,
                        )
                    } else {
                        super::graph::get_or_build_graph_topology_with_timings(
                            root,
                            &file_paths,
                            &registry,
                            opts.no_cache,
                            source_scope,
                            timings,
                        )
                    }
                },
                |g| g.entities.len(),
            );
            let entity = find_entity(
                &graph,
                opts.entity_name.as_deref(),
                opts.entity_id.as_deref(),
                file_hint.as_deref(),
            )?;
            timings.mark("entity_lookup");
            let mut report = ImpactReport::for_entity(entity.clone());
            report.dependents = graph
                .get_dependents(&entity.id)
                .into_iter()
                .cloned()
                .collect();
            report
        }
        ImpactMode::Tests | ImpactMode::All => {
            if file_paths.len() > LARGE_IMPACT_CACHE_MISS_FILE_THRESHOLD {
                let graph_data = build_with_spinner(
                    file_paths.len(),
                    || {
                        super::graph::get_or_build_graph_with_test_data_and_topology_save_on_miss_with_timings(
                            root,
                            &file_paths,
                            &registry,
                            opts.no_cache,
                            source_scope,
                            timings,
                        )
                    },
                    |gd| match gd {
                        super::graph::GraphWithTestData::Full(g, _) => g.entities.len(),
                        super::graph::GraphWithTestData::Topology { graph, .. } => {
                            graph.entities.len()
                        }
                    },
                );
                match graph_data {
                    super::graph::GraphWithTestData::Full(graph, all_entities) => {
                        let entity = find_entity(
                            &graph,
                            opts.entity_name.as_deref(),
                            opts.entity_id.as_deref(),
                            file_hint.as_deref(),
                        )?;
                        timings.mark("entity_lookup");
                        match opts.mode {
                            ImpactMode::Tests => report_tests(
                                &graph,
                                entity,
                                &all_entities,
                                &registry.custom_test_dirs,
                            ),
                            ImpactMode::All => report_all(
                                &graph,
                                entity,
                                &all_entities,
                                opts.depth,
                                &registry.custom_test_dirs,
                            ),
                            _ => unreachable!(),
                        }
                    }
                    super::graph::GraphWithTestData::Topology {
                        graph,
                        test_entity_ids,
                    } => {
                        let entity = find_entity(
                            &graph,
                            opts.entity_name.as_deref(),
                            opts.entity_id.as_deref(),
                            file_hint.as_deref(),
                        )?;
                        timings.mark("entity_lookup");
                        match opts.mode {
                            ImpactMode::Tests => {
                                report_tests_with_ids(&graph, entity, &test_entity_ids)
                            }
                            ImpactMode::All => {
                                report_all_with_ids(&graph, entity, &test_entity_ids, opts.depth)
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            } else {
                let (graph, all_entities) = build_with_spinner(
                    file_paths.len(),
                    || {
                        super::graph::get_or_build_graph_with_timings(
                            root,
                            &file_paths,
                            &registry,
                            opts.no_cache,
                            source_scope,
                            timings,
                        )
                    },
                    |(g, _)| g.entities.len(),
                );
                let entity = find_entity(
                    &graph,
                    opts.entity_name.as_deref(),
                    opts.entity_id.as_deref(),
                    file_hint.as_deref(),
                )?;
                timings.mark("entity_lookup");
                match opts.mode {
                    ImpactMode::Tests => {
                        report_tests(&graph, entity, &all_entities, &registry.custom_test_dirs)
                    }
                    ImpactMode::All => report_all(
                        &graph,
                        entity,
                        &all_entities,
                        opts.depth,
                        &registry.custom_test_dirs,
                    ),
                    _ => unreachable!(),
                }
            }
        }
    };
    Ok(ResolvedImpact::new(report, ImpactSource::Local))
}

fn render_and_finish_impact(resolved: &ResolvedImpact, opts: &ImpactOptions, mut timings: Timings) {
    if resolved.source == ImpactSource::Cloud && !timings.is_json() {
        super::cloud::show_cloud_banner();
    }
    print_impact_report(
        &resolved.report,
        resolved.source,
        opts.mode,
        opts.json,
        opts.depth,
    );
    timings.mark("cli_output_serialization");
    timings.source(resolved.source.as_str());
    timings.finish();
}

/// The wire shape of the sidecar's `impact` op — real serialized `EntityInfo`s,
/// so this deserializes into the backend-neutral report consumed by the same
/// renderer as local and cached results.
#[derive(serde::Deserialize)]
struct SidecarImpactResult {
    entity: EntityInfo,
    dependencies: Vec<EntityInfo>,
    dependents: Vec<EntityInfo>,
    impact: Vec<(EntityInfo, usize)>,
    tests: Vec<EntityInfo>,
}

/// Fast path: answer from the resident server's warm graph via its unix
/// socket, skipping this process's cache open + hydrate. Only for queries the
/// server can answer with identical semantics: default source scope, resolve
/// by name. Everything else — and any sidecar miss, error, or ambiguity —
/// falls back to the normal local path and its richer diagnostics.
fn try_sidecar_impact(opts: &ImpactOptions) -> Option<ImpactReport> {
    if opts.no_cache
        || opts.no_default_excludes
        || !opts.file_exts.is_empty()
        || opts.entity_id.is_some()
    {
        return None;
    }
    let Some(name) = opts.entity_name.as_deref() else {
        return None;
    };
    let Ok(git) = GitBridge::open(Path::new(&opts.cwd)) else {
        return None;
    };
    let root = git.repo_root().to_path_buf();
    // A .semignore means this repo's default scope is custom; the resident
    // server may not share it, so stay local (mirrors cache_source_scope).
    if root.join(".semignore").exists() {
        return None;
    }

    let mut request = serde_json::json!({ "op": "impact", "name": name, "depth": opts.depth });
    if let Some(file) = opts.file_hint.as_deref() {
        request["file"] = serde_json::json!(super::normalize_repo_relative_path(
            Path::new(&opts.cwd),
            &root,
            file
        ));
    }

    let Some(response) = super::sidecar::query(&root, &request) else {
        return None;
    };
    let Some(result) = response.get("result") else {
        return None;
    };
    let Ok(parsed) = serde_json::from_value::<SidecarImpactResult>(result.clone()) else {
        return None;
    };

    // An empty tests answer is not authoritative: graph edges miss
    // namespace-attribute calls ("xr.where"), and the full local path has a
    // lexical fallback for exactly that. Fall through instead of printing
    // "No tests found" from the fast path.
    if matches!(opts.mode, ImpactMode::Tests | ImpactMode::All) && parsed.tests.is_empty() {
        return None;
    }
    let report = ImpactReport {
        entity: parsed.entity,
        dependencies: parsed.dependencies,
        dependents: parsed.dependents,
        impact: parsed.impact,
        tests: parsed.tests,
        tests_truncated: false,
        test_evidence: Default::default(),
    };
    Some(report)
}

fn try_cached_impact_query(
    disk: &DiskCache,
    root: &Path,
    file_paths: &[String],
    opts: &ImpactOptions,
    file_hint: Option<&str>,
    source_scope: CacheSourceScope,
    cache_first: bool,
    timings: &mut Timings,
) -> Result<Option<ImpactReport>, ImpactQueryError> {
    match disk.query_impact_topology(
        root,
        file_paths,
        source_scope,
        cache_first,
        opts.entity_name.as_deref(),
        opts.entity_id.as_deref(),
        file_hint,
        cached_mode_for(opts.mode),
        opts.depth,
    ) {
        Ok(Some(result)) => {
            timings.mark("cache_topology_impact_query");
            // Empty tests from the cache is not authoritative (namespace
            // calls have no edges; older caches lack test flags/content).
            // Fall through to the full path, which has a lexical fallback.
            if matches!(opts.mode, ImpactMode::Tests) && result.tests.is_empty() {
                timings.mark("cache_tests_empty_fallthrough");
                return Ok(None);
            }
            Ok(Some(result))
        }
        Ok(None) => {
            timings.mark("cache_topology_impact_miss");
            Ok(None)
        }
        Err(ImpactQueryError::CacheReadFailed) => {
            timings.mark("cache_topology_impact_query_failed");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn cached_mode_for(mode: ImpactMode) -> CachedImpactMode {
    match mode {
        ImpactMode::All => CachedImpactMode::All,
        ImpactMode::Deps => CachedImpactMode::Deps,
        ImpactMode::Dependents => CachedImpactMode::Dependents,
        ImpactMode::Tests => CachedImpactMode::Tests,
    }
}

fn find_entity<'a>(
    graph: &'a EntityGraph,
    name: Option<&str>,
    entity_id: Option<&str>,
    file_hint: Option<&str>,
) -> Result<&'a EntityInfo, ImpactQueryError> {
    // Direct lookup by entity ID
    if let Some(id) = entity_id {
        if let Some(e) = graph.entities.get(id) {
            return Ok(e);
        }
        return Err(ImpactQueryError::EntityIdNotFound(id.to_string()));
    }

    let name = name.ok_or(ImpactQueryError::MissingEntityQuery)?;

    let mut matching: Vec<_> = graph
        .entities
        .values()
        .filter(|e| super::entity_matches_qualified(graph, e, name))
        .collect();

    if matching.is_empty() {
        return Err(ImpactQueryError::EntityNotFound(name.to_string()));
    }

    if let Some(file) = file_hint {
        let filtered: Vec<_> = matching
            .iter()
            .filter(|e| e.file_path == file)
            .copied()
            .collect();
        if filtered.len() == 1 {
            return Ok(filtered[0]);
        }
        if filtered.is_empty() {
            return Err(ImpactQueryError::EntityNotFoundInFile {
                name: name.to_string(),
                file: file.to_string(),
            });
        }
        // Multiple matches even within the file — fall through to ambiguity error
        matching = filtered;
    }

    if matching.len() == 1 {
        return Ok(matching[0]);
    }

    // Multiple matches — preserve the candidates for the presentation layer.
    matching.sort_by_key(|e| (&e.file_path, e.start_line));
    Err(ImpactQueryError::AmbiguousEntity {
        name: name.to_string(),
        matches: matching.into_iter().cloned().collect(),
    })
}

fn entity_json(e: &sem_core::parser::graph::EntityInfo) -> serde_json::Value {
    serde_json::json!({
        "entityId": e.id, "name": e.name, "type": e.entity_type,
        "file": e.file_path, "lines": [e.start_line, e.end_line],
    })
}

fn owned_entity_list_json(entities: &[EntityInfo]) -> Vec<serde_json::Value> {
    entities.iter().map(entity_json).collect()
}

fn print_entity_header(e: &sem_core::parser::graph::EntityInfo) {
    println!(
        "{} {} {} ({}:{}–{})",
        "⊕".green(),
        e.entity_type.dimmed(),
        e.name.bold(),
        e.file_path.dimmed(),
        e.start_line,
        e.end_line,
    );
}

fn print_impact_report(
    result: &ImpactReport,
    source: ImpactSource,
    mode: ImpactMode,
    json: bool,
    depth: usize,
) {
    if source == ImpactSource::Cloud {
        print_cloud_impact_report(result, mode, json);
        return;
    }
    match mode {
        ImpactMode::Deps => {
            print_dependencies(&result.entity, &result.dependencies, json);
        }
        ImpactMode::Dependents => {
            print_dependents(&result.entity, &result.dependents, json);
        }
        ImpactMode::Tests => {
            if result.test_evidence == TestEvidence::LexicalFallback && !json {
                println!(
                    "{}",
                    "  (no call-graph edges reach tests; lexical fallback — test bodies naming the entity)"
                        .dimmed()
                );
            }
            print_tests(&result.entity, &result.tests, result.tests_truncated, json);
        }
        ImpactMode::All => {
            print_all(result, json, depth);
        }
    }
}

fn cloud_entity_json(entity: &EntityInfo) -> serde_json::Value {
    serde_json::json!({
        "entityId": entity.id,
        "name": entity.name,
        "type": entity.entity_type,
        "file": entity.file_path,
    })
}

fn print_cloud_impact_report(result: &ImpactReport, mode: ImpactMode, json: bool) {
    let target_json = || {
        serde_json::json!({
            "name": result.entity.name,
            "file": result.entity.file_path,
        })
    };
    let entities_json =
        |entities: &[EntityInfo]| entities.iter().map(cloud_entity_json).collect::<Vec<_>>();
    let print_header = || {
        println!(
            "{} {}{}",
            "⊕".green(),
            result.entity.name.bold(),
            if result.entity.file_path.is_empty() {
                String::new()
            } else {
                format!(" ({})", result.entity.file_path.dimmed())
            },
        );
    };
    let print_dependencies = || {
        if !result.dependencies.is_empty() {
            println!("\n  {} {}", "→".blue(), "depends on:".dimmed());
            for dependency in &result.dependencies {
                println!(
                    "    {} {} {} ({})",
                    "→".blue(),
                    dependency.entity_type.dimmed(),
                    dependency.name.bold(),
                    dependency.file_path.dimmed(),
                );
            }
        }
    };
    let print_dependents = || {
        if !result.dependents.is_empty() {
            println!("\n  {} {}", "←".yellow(), "depended on by:".dimmed());
            for dependent in &result.dependents {
                println!(
                    "    {} {} {} ({})",
                    "←".yellow(),
                    dependent.entity_type.dimmed(),
                    dependent.name.bold(),
                    dependent.file_path.dimmed(),
                );
            }
        }
    };

    match mode {
        ImpactMode::Deps => {
            if json {
                let output = serde_json::json!({
                    "entity": target_json(),
                    "dependencies": entities_json(&result.dependencies),
                });
                println!("{}", serde_json::to_string(&output).unwrap());
            } else {
                print_header();
                if result.dependencies.is_empty() {
                    println!("\n  {} {}", "✓".green().bold(), "No dependencies.".dimmed());
                } else {
                    print_dependencies();
                }
                println!();
            }
        }
        ImpactMode::Dependents => {
            if json {
                let output = serde_json::json!({
                    "entity": target_json(),
                    "dependents": entities_json(&result.dependents),
                });
                println!("{}", serde_json::to_string(&output).unwrap());
            } else {
                print_header();
                if result.dependents.is_empty() {
                    println!("\n  {} {}", "✓".green().bold(), "No dependents.".dimmed());
                } else {
                    print_dependents();
                }
                println!();
            }
        }
        ImpactMode::All => {
            if json {
                let impact = result
                    .impact
                    .iter()
                    .map(|(entity, _)| cloud_entity_json(entity))
                    .collect::<Vec<_>>();
                let output = serde_json::json!({
                    "entity": target_json(),
                    "dependencies": entities_json(&result.dependencies),
                    "dependents": entities_json(&result.dependents),
                    "impact": {
                        "total": impact.len(),
                        "entities": impact,
                    },
                    "tests": [],
                });
                println!("{}", serde_json::to_string(&output).unwrap());
                return;
            }

            print_header();
            print_dependencies();
            print_dependents();
            if result.impact.is_empty() {
                if result.dependencies.is_empty() && result.dependents.is_empty() {
                    println!(
                        "\n  {} {}",
                        "✓".green().bold(),
                        "No dependencies or dependents found.".dimmed()
                    );
                }
            } else {
                println!(
                    "\n  {} {}",
                    "!".red().bold(),
                    format!("{} entities transitively affected:", result.impact.len()).red(),
                );
                for (entity, _) in &result.impact {
                    println!(
                        "    {} {} {} ({})",
                        "→".red(),
                        entity.entity_type.dimmed(),
                        entity.name.bold(),
                        entity.file_path.dimmed(),
                    );
                }
            }
            println!();
        }
        ImpactMode::Tests => unreachable!("cloud impact does not support test queries"),
    }
}

fn print_dependencies(entity: &EntityInfo, deps: &[EntityInfo], json: bool) {
    if json {
        let output = serde_json::json!({
            "entity": entity_json(entity),
            "dependencies": owned_entity_list_json(deps),
        });
        println!("{}", serde_json::to_string(&output).unwrap());
    } else {
        print_entity_header(entity);
        if deps.is_empty() {
            println!("\n  {} {}", "✓".green().bold(), "No dependencies.".dimmed());
        } else {
            println!("\n  {} {}", "→".blue(), "depends on:".dimmed());
            for dep in deps {
                println!(
                    "    {} {} {} ({})",
                    "→".blue(),
                    dep.entity_type.dimmed(),
                    dep.name.bold(),
                    dep.file_path.dimmed(),
                );
            }
        }
        println!();
    }
}

fn print_dependents(entity: &EntityInfo, dependents: &[EntityInfo], json: bool) {
    if json {
        let output = serde_json::json!({
            "entity": entity_json(entity),
            "dependents": owned_entity_list_json(dependents),
        });
        println!("{}", serde_json::to_string(&output).unwrap());
    } else {
        print_entity_header(entity);
        if dependents.is_empty() {
            println!("\n  {} {}", "✓".green().bold(), "No dependents.".dimmed());
        } else {
            println!("\n  {} {}", "←".yellow(), "depended on by:".dimmed());
            for dep in dependents {
                println!(
                    "    {} {} {} ({})",
                    "←".yellow(),
                    dep.entity_type.dimmed(),
                    dep.name.bold(),
                    dep.file_path.dimmed(),
                );
            }
        }
        println!();
    }
}

fn print_tests(entity: &EntityInfo, tests: &[EntityInfo], truncated: bool, json: bool) {
    if json {
        let mut output = serde_json::json!({
            "entity": entity_json(entity),
            "tests": owned_entity_list_json(tests),
        });
        if truncated {
            output
                .as_object_mut()
                .unwrap()
                .insert("testsTruncated".to_string(), serde_json::json!(true));
        }
        println!("{}", serde_json::to_string(&output).unwrap());
    } else {
        print_entity_header(entity);
        if tests.is_empty() {
            println!("\n  {} {}", "✓".green().bold(), "No tests found.".dimmed());
        } else {
            println!(
                "\n  {} {}",
                "⚡".yellow(),
                format!("{} tests affected:", tests.len()).bold()
            );
            let mut by_file: std::collections::HashMap<&str, Vec<_>> =
                std::collections::HashMap::new();
            for test in tests {
                by_file
                    .entry(test.file_path.as_str())
                    .or_default()
                    .push(test);
            }
            let mut files: Vec<_> = by_file.keys().copied().collect();
            files.sort();
            for file in files {
                println!("    {}", file.bold());
                let mut entities = by_file[file].clone();
                entities.sort_by_key(|test| test.start_line);
                for test in entities {
                    println!(
                        "      {} {} (L{}–{})",
                        test.entity_type.dimmed(),
                        test.name.bold(),
                        test.start_line,
                        test.end_line,
                    );
                }
            }
        }
        print_tests_truncation_warning(truncated);
        println!();
    }
}

fn print_tests_truncation_warning(truncated: bool) {
    if truncated {
        println!(
            "\n  {} {}",
            "warning:".yellow().bold(),
            "Cached test impact reached its traversal limit; results may be incomplete.".yellow()
        );
    }
}

fn print_all(result: &ImpactReport, json: bool, depth: usize) {
    if json {
        let impact_entities: Vec<serde_json::Value> = result
            .impact
            .iter()
            .map(|(entity, depth)| {
                let mut value = entity_json(entity);
                value
                    .as_object_mut()
                    .unwrap()
                    .insert("depth".to_string(), serde_json::json!(depth));
                value
            })
            .collect();
        let mut output = serde_json::json!({
            "entity": entity_json(&result.entity),
            "dependencies": owned_entity_list_json(&result.dependencies),
            "dependents": owned_entity_list_json(&result.dependents),
            "impact": {
                "depth": depth,
                "total": result.impact.len(),
                "entities": impact_entities,
            },
            "tests": owned_entity_list_json(&result.tests),
        });
        if result.tests_truncated {
            output
                .as_object_mut()
                .unwrap()
                .insert("testsTruncated".to_string(), serde_json::json!(true));
        }
        println!("{}", serde_json::to_string(&output).unwrap());
        return;
    }

    print_entity_header(&result.entity);

    if !result.dependencies.is_empty() {
        println!("\n  {} {}", "→".blue(), "depends on:".dimmed());
        for dep in &result.dependencies {
            println!(
                "    {} {} {} ({})",
                "→".blue(),
                dep.entity_type.dimmed(),
                dep.name.bold(),
                dep.file_path.dimmed(),
            );
        }
    }

    if !result.dependents.is_empty() {
        println!("\n  {} {}", "←".yellow(), "depended on by:".dimmed());
        for dep in &result.dependents {
            println!(
                "    {} {} {} ({})",
                "←".yellow(),
                dep.entity_type.dimmed(),
                dep.name.bold(),
                dep.file_path.dimmed(),
            );
        }
    }

    if result.impact.is_empty() {
        println!(
            "\n  {} {}",
            "✓".green().bold(),
            "No other entities are affected by changes to this entity.".dimmed()
        );
    } else {
        let max_depth_seen = result
            .impact
            .iter()
            .map(|(_, depth)| *depth)
            .max()
            .unwrap_or(0);
        let depth_label = if depth == 0 {
            "unlimited".to_string()
        } else {
            format!("depth {}", depth)
        };
        println!(
            "\n  {} {}",
            "!".red().bold(),
            format!(
                "{} entities transitively affected ({}):",
                result.impact.len(),
                depth_label
            )
            .red(),
        );

        for current_depth in 1..=max_depth_seen {
            let at_depth: Vec<_> = result
                .impact
                .iter()
                .filter(|(_, depth)| *depth == current_depth)
                .map(|(entity, _)| entity)
                .collect();
            if at_depth.is_empty() {
                continue;
            }

            let label = if current_depth == 1 {
                "Direct dependents".to_string()
            } else {
                format!("Depth {}", current_depth)
            };
            println!("\n    {} ({})", label.bold(), at_depth.len());
            for entity in at_depth {
                println!(
                    "      {} {} {} ({}:L{})",
                    "→".red(),
                    entity.entity_type.dimmed(),
                    entity.name.bold(),
                    entity.file_path.dimmed(),
                    entity.start_line,
                );
            }
        }
    }

    if !result.tests.is_empty() {
        println!(
            "\n  {} {}",
            "⚡".yellow(),
            format!("{} tests affected:", result.tests.len()).bold()
        );
        for test in &result.tests {
            println!(
                "    {} {} ({})",
                test.entity_type.dimmed(),
                test.name.bold(),
                test.file_path.dimmed(),
            );
        }
    }
    print_tests_truncation_warning(result.tests_truncated);

    println!();
}

fn print_impact_error(error: ImpactQueryError) -> ! {
    match error {
        ImpactQueryError::CacheReadFailed => {
            eprintln!(
                "{} Failed to read the cached impact graph",
                "error:".red().bold()
            );
        }
        ImpactQueryError::MissingEntityQuery => {
            eprintln!(
                "{} Either entity name or --entity-id is required",
                "error:".red().bold()
            );
        }
        ImpactQueryError::EntityIdNotFound(id) => {
            eprintln!("{} Entity ID '{}' not found", "error:".red().bold(), id);
        }
        ImpactQueryError::EntityNotFound(name) => {
            eprintln!("{} Entity '{}' not found", "error:".red().bold(), name);
        }
        ImpactQueryError::EntityNotFoundInFile { name, file } => {
            eprintln!(
                "{} Entity '{}' not found in file '{}'",
                "error:".red().bold(),
                name,
                file
            );
        }
        ImpactQueryError::AmbiguousEntity { name, mut matches } => {
            matches.sort_by_key(|entity| {
                (
                    entity.file_path.clone(),
                    entity.start_line,
                    entity.id.clone(),
                )
            });
            eprintln!(
                "{} Entity name '{}' is ambiguous ({} matches). Specify --file or --entity-id:",
                "error:".red().bold(),
                name,
                matches.len()
            );
            for entity in &matches {
                eprintln!(
                    "  {} {} ({}:L{})",
                    entity.entity_type, entity.id, entity.file_path, entity.start_line
                );
            }
        }
    }
    std::process::exit(1);
}

fn report_tests(
    graph: &EntityGraph,
    entity: &EntityInfo,
    all_entities: &[sem_core::model::entity::SemanticEntity],
    custom_test_dirs: &[String],
) -> ImpactReport {
    let mut report = ImpactReport::for_entity(entity.clone());
    let tests = graph.test_impact_with_custom_dirs(&entity.id, all_entities, custom_test_dirs);
    if !tests.is_empty() {
        report.tests = tests.into_iter().cloned().collect();
        return report;
    }
    // Graph edges can miss tests that call the target through a module
    // namespace ("xr.where(...)"): the attribute call resolves to no entity.
    // Fall back to lexical reachability — test bodies naming the entity as a
    // word — and say so, since it is weaker evidence than a call edge.
    let test_ids = graph.filter_test_entities_with_custom_dirs(all_entities, custom_test_dirs);
    report.tests = all_entities
        .iter()
        .filter(|e| test_ids.contains(&e.id) && word_hit(&e.content, &entity.name))
        .map(|e| EntityInfo {
            id: e.id.clone(),
            name: e.name.clone(),
            entity_type: e.entity_type.clone(),
            file_path: e.file_path.clone(),
            parent_id: e.parent_id.clone(),
            start_line: e.start_line,
            end_line: e.end_line,
        })
        .collect();
    if !report.tests.is_empty() {
        report.test_evidence = TestEvidence::LexicalFallback;
    }
    report
}

/// True when `name` appears in `body` as a whole word (not as a substring of
/// a longer identifier).
pub(crate) fn word_hit(body: &str, name: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = body[start..].find(name) {
        let abs = start + pos;
        let before_ok = abs == 0
            || !body[..abs]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after = abs + name.len();
        let after_ok = after >= body.len()
            || !body[after..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if before_ok && after_ok {
            return true;
        }
        start = abs + name.len();
    }
    false
}

fn report_tests_with_ids(
    graph: &EntityGraph,
    entity: &EntityInfo,
    test_entity_ids: &HashSet<String>,
) -> ImpactReport {
    let mut report = ImpactReport::for_entity(entity.clone());
    report.tests = test_impact_from_ids(graph, &entity.id, test_entity_ids)
        .into_iter()
        .cloned()
        .collect();
    report
}

fn report_all(
    graph: &EntityGraph,
    entity: &EntityInfo,
    all_entities: &[sem_core::model::entity::SemanticEntity],
    depth: usize,
    custom_test_dirs: &[String],
) -> ImpactReport {
    let tests = graph.test_impact_with_custom_dirs(&entity.id, all_entities, custom_test_dirs);
    report_all_with_tests(graph, entity, &tests, depth)
}

fn report_all_with_ids(
    graph: &EntityGraph,
    entity: &EntityInfo,
    test_entity_ids: &HashSet<String>,
    depth: usize,
) -> ImpactReport {
    let tests = test_impact_from_ids(graph, &entity.id, test_entity_ids);
    report_all_with_tests(graph, entity, &tests, depth)
}

fn test_impact_from_ids<'a>(
    graph: &'a EntityGraph,
    entity_id: &str,
    test_entity_ids: &HashSet<String>,
) -> Vec<&'a EntityInfo> {
    graph
        .impact_analysis(entity_id)
        .into_iter()
        .filter(|info| test_entity_ids.contains(&info.id))
        .collect()
}

fn report_all_with_tests(
    graph: &EntityGraph,
    entity: &EntityInfo,
    tests: &[&EntityInfo],
    depth: usize,
) -> ImpactReport {
    let mut report = ImpactReport::for_entity(entity.clone());
    report.dependencies = graph
        .get_dependencies(&entity.id)
        .into_iter()
        .cloned()
        .collect();
    report.dependents = graph
        .get_dependents(&entity.id)
        .into_iter()
        .cloned()
        .collect();
    report.impact = graph
        .impact_analysis_bounded(&entity.id, depth)
        .into_iter()
        .map(|(entity, depth)| (EntityInfo::clone(entity), depth))
        .collect();
    report.tests = tests
        .iter()
        .map(|entity| EntityInfo::clone(*entity))
        .collect();
    report
}
