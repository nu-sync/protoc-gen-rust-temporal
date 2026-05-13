//! Cross-method invariants applied after `parse.rs` builds a `ServiceModel`.
//!
//! Errors here translate directly into `CodeGeneratorResponse.error` and
//! surface to the user as `protoc` failures, so messages should pinpoint
//! the service + rpc + offending option.

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};

use crate::model::ServiceModel;

pub fn validate(model: &ServiceModel, _options: &crate::options::RenderOptions) -> Result<()> {
    reject_rpc_collisions(model)?;
    reject_workflow_alias_collisions_across_workflows(model)?;
    reject_handler_registered_name_collisions(model)?;
    reject_conflicting_ref_cli_overrides(model)?;
    reject_workflow_cli_name_collisions(model)?;
    validate_workflows(model)?;
    validate_signal_outputs(model)?;
    validate_empty_with_start(model)?;
    Ok(())
}

/// Reject when two workflows on the same service declare the same
/// `(temporal.v1.workflow).cli.name` (or the same entry in
/// `cli.aliases`). They'd produce identical clap subcommand names
/// (`start-<value>` / `attach-<value>` / `cancel-<value>` /
/// `terminate-<value>`) on the CLI scaffold, and clap rejects
/// duplicate subcommand names at runtime — better to surface the
/// conflict at codegen with the workflow names called out.
fn reject_workflow_cli_name_collisions(model: &ServiceModel) -> Result<()> {
    // Each CLI subcommand value maps to its owning workflow; first
    // insertion wins, the second is the duplicate-subcommand bug.
    let mut owners: HashMap<&str, &str> = HashMap::new();
    for wf in &model.workflows {
        if let Some(name) = wf.cli_name.as_deref() {
            if let Some(prior) = owners.insert(name, wf.rpc_method.as_str()) {
                bail!(
                    "{service}: cli subcommand value `{name}` is declared by both `{prior}` and `{owner}` — clap would reject the duplicate at runtime; reconcile the cli.name / cli.aliases values",
                    service = model.service,
                    owner = wf.rpc_method,
                );
            }
        }
        for alias in &wf.cli_aliases {
            if let Some(prior) = owners.insert(alias.as_str(), wf.rpc_method.as_str()) {
                bail!(
                    "{service}: cli subcommand value `{alias}` is declared by both `{prior}` and `{owner}` — clap would reject the duplicate at runtime; reconcile the cli.name / cli.aliases values",
                    service = model.service,
                    owner = wf.rpc_method,
                );
            }
        }
    }
    Ok(())
}

/// Reject when multiple workflows declare contradictory
/// `cli.{name,aliases,usage}` overrides for the same signal or
/// update ref. The CLI emit is service-scoped — there's only one
/// `Signal<Name>` / `Update<Name>` variant per handler regardless of
/// how many workflows ref it — so render picks the first override
/// it sees and silently drops the rest. Contradictory user intent
/// would surface as "why did my CLI subcommand pick that name?"
/// only at runtime; reject at codegen instead.
fn reject_conflicting_ref_cli_overrides(model: &ServiceModel) -> Result<()> {
    // For each signal rpc, collect every (workflow, override-tuple)
    // pair that declares overrides. If any pair disagrees on any
    // axis (name / aliases / usage), bail.
    #[derive(PartialEq, Eq)]
    struct Override<'a> {
        name: Option<&'a str>,
        aliases: &'a [String],
        usage: Option<&'a str>,
    }
    fn check<'a, Ref>(
        model: &ServiceModel,
        kind: &str,
        refs_by_workflow: &[(&'a str, &'a [Ref])],
        rpc_method: fn(&Ref) -> &str,
        as_override: fn(&Ref) -> Option<Override<'_>>,
    ) -> Result<()> {
        let mut by_target: HashMap<&str, Vec<(&str, Override<'_>)>> = HashMap::new();
        for (wf_name, refs) in refs_by_workflow {
            for r in refs.iter() {
                let Some(ov) = as_override(r) else { continue };
                by_target
                    .entry(rpc_method(r))
                    .or_default()
                    .push((wf_name, ov));
            }
        }
        for (target, owners) in &by_target {
            if owners.len() < 2 {
                continue;
            }
            let first = &owners[0].1;
            for (wf_name, ov) in &owners[1..] {
                if ov != first {
                    bail!(
                        "{service}: {kind} ref `{target}` carries contradictory cli overrides across workflows (`{a}` and `{b}`); service-scoped CLI emit picks the first override silently — reconcile the overrides or remove duplicates",
                        service = model.service,
                        a = owners[0].0,
                        b = wf_name,
                    );
                }
            }
        }
        Ok(())
    }

    let signal_refs: Vec<(&str, &[crate::model::SignalRef])> = model
        .workflows
        .iter()
        .map(|wf| (wf.rpc_method.as_str(), wf.attached_signals.as_slice()))
        .collect();
    check::<crate::model::SignalRef>(
        model,
        "signal",
        &signal_refs,
        |r| r.rpc_method.as_str(),
        |r| {
            if r.cli_name.is_none() && r.cli_aliases.is_empty() && r.cli_usage.is_none() {
                return None;
            }
            Some(Override {
                name: r.cli_name.as_deref(),
                aliases: r.cli_aliases.as_slice(),
                usage: r.cli_usage.as_deref(),
            })
        },
    )?;

    let update_refs: Vec<(&str, &[crate::model::UpdateRef])> = model
        .workflows
        .iter()
        .map(|wf| (wf.rpc_method.as_str(), wf.attached_updates.as_slice()))
        .collect();
    check::<crate::model::UpdateRef>(
        model,
        "update",
        &update_refs,
        |r| r.rpc_method.as_str(),
        |r| {
            if r.cli_name.is_none() && r.cli_aliases.is_empty() && r.cli_usage.is_none() {
                return None;
            }
            Some(Override {
                name: r.cli_name.as_deref(),
                aliases: r.cli_aliases.as_slice(),
                usage: r.cli_usage.as_deref(),
            })
        },
    )?;
    Ok(())
}

/// Two activities (or signals, queries, updates) on the same service
/// cannot share a `registered_name` — workers would silently dispatch
/// one or the other depending on registration order. Cross-kind
/// collisions are fine (workflow "Foo" + signal "Foo" are distinct
/// Temporal namespaces); we only enforce intra-kind uniqueness here.
/// Workflow registered_name collisions are caught by the broader
/// alias-collision check above.
fn reject_handler_registered_name_collisions(model: &ServiceModel) -> Result<()> {
    fn check_kind<'a, I, F>(model: &ServiceModel, kind: &str, items: I, get: F) -> Result<()>
    where
        I: IntoIterator<Item = &'a (dyn HandlerName + 'a)>,
        F: Fn(&dyn HandlerName) -> &str,
    {
        let _ = get; // kept for symmetry with the closure-based pattern.
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for item in items {
            let name = item.registered_name();
            if let Some(prior) = seen.insert(name, item.rpc_method()) {
                bail!(
                    "{}: two distinct {kind} rpcs (`{prior}` and `{later}`) register under the same Temporal name `{name}` — rename one or remove the duplicate",
                    model.service,
                    later = item.rpc_method(),
                );
            }
        }
        Ok(())
    }
    let acts: Vec<&dyn HandlerName> = model
        .activities
        .iter()
        .map(|a| a as &dyn HandlerName)
        .collect();
    let sigs: Vec<&dyn HandlerName> = model
        .signals
        .iter()
        .map(|s| s as &dyn HandlerName)
        .collect();
    let qs: Vec<&dyn HandlerName> = model
        .queries
        .iter()
        .map(|q| q as &dyn HandlerName)
        .collect();
    let us: Vec<&dyn HandlerName> = model
        .updates
        .iter()
        .map(|u| u as &dyn HandlerName)
        .collect();
    check_kind(model, "activity", acts.iter().copied(), |h| {
        h.registered_name()
    })?;
    check_kind(model, "signal", sigs.iter().copied(), |h| {
        h.registered_name()
    })?;
    check_kind(model, "query", qs.iter().copied(), |h| h.registered_name())?;
    check_kind(model, "update", us.iter().copied(), |h| h.registered_name())?;
    Ok(())
}

/// Small trait so the registered-name collision check can iterate
/// over each handler kind uniformly without duplicating the loop body.
trait HandlerName {
    fn rpc_method(&self) -> &str;
    fn registered_name(&self) -> &str;
}

impl HandlerName for crate::model::ActivityModel {
    fn rpc_method(&self) -> &str {
        &self.rpc_method
    }
    fn registered_name(&self) -> &str {
        &self.registered_name
    }
}
impl HandlerName for crate::model::SignalModel {
    fn rpc_method(&self) -> &str {
        &self.rpc_method
    }
    fn registered_name(&self) -> &str {
        &self.registered_name
    }
}
impl HandlerName for crate::model::QueryModel {
    fn rpc_method(&self) -> &str {
        &self.rpc_method
    }
    fn registered_name(&self) -> &str {
        &self.registered_name
    }
}
impl HandlerName for crate::model::UpdateModel {
    fn rpc_method(&self) -> &str {
        &self.rpc_method
    }
    fn registered_name(&self) -> &str {
        &self.registered_name
    }
}

/// Reject when two workflows on the same service register under the
/// same Temporal name via overlapping `aliases` — or when an alias on
/// one workflow collides with another workflow's `registered_name`.
/// Either case attempts duplicate registration at runtime, so refuse
/// at codegen. Self-collisions and intra-list duplicates are caught
/// earlier in parse (`workflow_from`).
fn reject_workflow_alias_collisions_across_workflows(model: &ServiceModel) -> Result<()> {
    // Map every Temporal-registered name (registered_name + each alias)
    // back to the workflow that registers it. First insertion wins; the
    // second insertion is the collision.
    let mut owners: HashMap<&str, (&str, &'static str)> = HashMap::new();
    for wf in &model.workflows {
        if let Some((prior_owner, prior_kind)) = owners.insert(
            wf.registered_name.as_str(),
            (wf.rpc_method.as_str(), "name"),
        ) {
            // Two workflows declared the same `registered_name` (either
            // both omitted `name` and snake-case'd into the same default,
            // or set the same explicit `name`).
            bail!(
                "workflow alias collision in `{service}`: both `{owner_a}` and `{owner_b}` register the Temporal name `{name}` as their {prior_kind}; rename one workflow or remove the duplicate",
                service = model.service,
                owner_a = prior_owner,
                owner_b = wf.rpc_method,
                name = wf.registered_name,
            );
        }
        for alias in &wf.aliases {
            if let Some((prior_owner, prior_kind)) =
                owners.insert(alias.as_str(), (wf.rpc_method.as_str(), "alias"))
            {
                bail!(
                    "workflow alias collision in `{service}`: alias `{alias}` on `{owner_b}` collides with the {prior_kind} of `{owner_a}`; aliases must be unique across all workflows on the service",
                    service = model.service,
                    owner_a = prior_owner,
                    owner_b = wf.rpc_method,
                );
            }
        }
    }
    Ok(())
}

/// Reject method-name collisions across **distinct rpcs**. Two different
/// proto rpcs that happen to share a snake-case name would collide on
/// generated symbols. Co-annotations on a **single** rpc (parse.rs allows
/// `activity` + one of workflow/signal/update) intentionally let the same
/// method name appear in multiple buckets; that's safe because the activity
/// emit lives in a separate trait surface that doesn't collide with the
/// client / handler emit.
fn reject_rpc_collisions(model: &ServiceModel) -> Result<()> {
    // Track which (name, kind) tuples we've seen. Within the *same* rpc,
    // co-annotations are allowed — the activity bucket is the only one
    // that may co-occur, and it can co-occur with any one other kind.
    // So the only collision we still reject is two non-activity entries
    // for the same name (e.g. a workflow rpc named `Cancel` plus a signal
    // rpc *also* named `Cancel` — two separate proto methods sharing a name).
    let mut by_name: HashMap<&str, Vec<&'static str>> = HashMap::new();
    for w in &model.workflows {
        by_name
            .entry(w.rpc_method.as_str())
            .or_default()
            .push("workflow");
    }
    for s in &model.signals {
        by_name
            .entry(s.rpc_method.as_str())
            .or_default()
            .push("signal");
    }
    for q in &model.queries {
        by_name
            .entry(q.rpc_method.as_str())
            .or_default()
            .push("query");
    }
    for u in &model.updates {
        by_name
            .entry(u.rpc_method.as_str())
            .or_default()
            .push("update");
    }
    for a in &model.activities {
        by_name
            .entry(a.rpc_method.as_str())
            .or_default()
            .push("activity");
    }

    for (name, kinds) in &by_name {
        let non_activity = kinds.iter().filter(|k| **k != "activity").count();
        if non_activity > 1 {
            bail!(
                "{}.{name}: distinct rpcs share a method name across non-activity buckets ({}) — generated symbols would collide; rename one of them",
                model.service,
                kinds.join(" + "),
            );
        }
    }
    Ok(())
}

fn validate_workflows(model: &ServiceModel) -> Result<()> {
    let signal_methods: HashSet<&str> = model
        .signals
        .iter()
        .map(|s| s.rpc_method.as_str())
        .collect();
    let query_methods: HashSet<&str> = model
        .queries
        .iter()
        .map(|q| q.rpc_method.as_str())
        .collect();
    let update_methods: HashSet<&str> = model
        .updates
        .iter()
        .map(|u| u.rpc_method.as_str())
        .collect();

    for wf in &model.workflows {
        let effective_tq = wf
            .task_queue
            .as_deref()
            .or(model.default_task_queue.as_deref());
        if effective_tq.is_none() {
            bail!(
                "{}.{}: workflow has no task_queue — set either (temporal.v1.workflow).task_queue or service-level (temporal.v1.service).task_queue",
                model.service,
                wf.rpc_method,
            );
        }

        for sref in &wf.attached_signals {
            // Cross-service refs already validated their target through
            // the DescriptorPool at parse-time (see
            // `resolve_cross_service_ref`); skip the same-service
            // existence check.
            if sref.cross_service.is_some() {
                continue;
            }
            check_ref(
                model,
                wf,
                &signal_methods,
                &sref.rpc_method,
                "signal",
                "(temporal.v1.signal)",
            )?;
        }
        for qref in &wf.attached_queries {
            if qref.cross_service.is_some() {
                continue;
            }
            check_ref(
                model,
                wf,
                &query_methods,
                &qref.rpc_method,
                "query",
                "(temporal.v1.query)",
            )?;
        }
        for uref in &wf.attached_updates {
            if uref.cross_service.is_some() {
                continue;
            }
            check_ref(
                model,
                wf,
                &update_methods,
                &uref.rpc_method,
                "update",
                "(temporal.v1.update)",
            )?;
        }
    }
    Ok(())
}

fn check_ref(
    model: &ServiceModel,
    wf: &crate::model::WorkflowModel,
    declared: &HashSet<&str>,
    target: &str,
    kind: &str,
    expected_annotation: &str,
) -> Result<()> {
    if declared.contains(target) {
        return Ok(());
    }
    // Dotted refs that reach here didn't carry resolved cross-service
    // metadata — that shouldn't happen post-2026-05-13 because parse.rs
    // either resolves them or fails. This branch stays as a defensive
    // diagnostic in case future refactors drop the parse-time resolution.
    if target.contains('.') {
        bail!(
            "{}.{}: workflow references {kind} \"{target}\" using a fully-qualified path but parse-time resolution didn't capture cross-service metadata — this is a plugin bug (parse.rs::resolve_cross_service_ref should have populated `{kind}_ref.cross_service`).",
            model.service,
            wf.rpc_method,
        );
    }
    bail!(
        "{}.{}: workflow references {kind} \"{target}\" but no sibling rpc carries {expected_annotation}",
        model.service,
        wf.rpc_method,
    );
}

/// `signal_with_start` / `update_with_start` free functions take both the
/// workflow input and the signal/update input. Emitting them generically
/// over Empty would require a combinatorial set of runtime functions or
/// a `TypedPayload` adapter we don't ship yet. Reject the combination
/// up front with a clear error so users wrap empty messages in a no-field
/// struct (the canonical proto workaround).
fn validate_empty_with_start(model: &ServiceModel) -> Result<()> {
    for wf in &model.workflows {
        for sref in &wf.attached_signals {
            if !sref.start {
                continue;
            }
            let Some(sig) = model
                .signals
                .iter()
                .find(|s| s.rpc_method == sref.rpc_method)
            else {
                continue; // unresolved ref — caught earlier
            };
            if wf.input_type.is_empty || sig.input_type.is_empty {
                bail!(
                    "{}.{}: signal `{}` is marked start:true but {} input is google.protobuf.Empty; the with_start emit path doesn't support Empty payloads. Wrap the empty side in a single-field message and retry.",
                    model.service,
                    wf.rpc_method,
                    sig.rpc_method,
                    if wf.input_type.is_empty {
                        "the workflow's"
                    } else {
                        "the signal's"
                    },
                );
            }
        }
        for uref in &wf.attached_updates {
            if !uref.start {
                continue;
            }
            let Some(u) = model
                .updates
                .iter()
                .find(|u| u.rpc_method == uref.rpc_method)
            else {
                continue;
            };
            if wf.input_type.is_empty || u.input_type.is_empty {
                bail!(
                    "{}.{}: update `{}` is marked start:true but {} input is google.protobuf.Empty; the with_start emit path doesn't support Empty payloads. Wrap the empty side in a single-field message and retry.",
                    model.service,
                    wf.rpc_method,
                    u.rpc_method,
                    if wf.input_type.is_empty {
                        "the workflow's"
                    } else {
                        "the update's"
                    },
                );
            }
        }
    }
    Ok(())
}

fn validate_signal_outputs(model: &ServiceModel) -> Result<()> {
    for sig in &model.signals {
        if !sig.output_type.is_empty {
            bail!(
                "{}.{}: signal rpc must return google.protobuf.Empty, got {}",
                model.service,
                sig.rpc_method,
                sig.output_type.full_name,
            );
        }
    }
    Ok(())
}
