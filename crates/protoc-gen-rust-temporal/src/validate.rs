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
    reject_unprintable_registered_names(model)?;
    reject_unusable_cli_overrides(model)?;
    validate_workflows(model)?;
    validate_signal_outputs(model)?;
    validate_empty_with_start(model)?;
    Ok(())
}

/// Reject CLI override values that clap can't accept as subcommand
/// names: empty strings, or anything containing whitespace / control
/// characters. clap parses subcommand tokens off the shell command
/// line — a value with a space splits into two args at runtime.
/// Diagnostics name the override site (workflow / signal-ref /
/// update-ref / service-level / method-level) and the bad character
/// so authors don't have to guess.
fn reject_unusable_cli_overrides(model: &ServiceModel) -> Result<()> {
    fn check(model_service: &str, site: &str, value: &str) -> Result<()> {
        if value.is_empty() {
            bail!(
                "{model_service}: {site} cli override is the empty string — clap cannot use an empty subcommand name; remove the field or set a non-empty value",
            );
        }
        for c in value.chars() {
            if c.is_whitespace() || c.is_control() {
                bail!(
                    "{model_service}: {site} cli override {value:?} contains whitespace / control character `{c:?}` — clap subcommand names must be a single shell token",
                );
            }
        }
        Ok(())
    }
    // Service-level (temporal.v1.cli) — name + each alias.
    if let Some(spec) = model.cli_options.as_ref() {
        if let Some(name) = spec.name.as_deref() {
            check(&model.service, "service-level cli.name", name)?;
        }
        for alias in &spec.aliases {
            check(&model.service, "service-level cli.aliases entry", alias)?;
        }
    }
    // Per-workflow (temporal.v1.workflow).cli.
    for wf in &model.workflows {
        if let Some(name) = wf.cli_name.as_deref() {
            check(
                &model.service,
                &format!("workflow `{}` cli.name", wf.rpc_method),
                name,
            )?;
        }
        for alias in &wf.cli_aliases {
            check(
                &model.service,
                &format!("workflow `{}` cli.aliases entry", wf.rpc_method),
                alias,
            )?;
        }
        // Per-signal-ref (WorkflowOptions.signal[N].cli).
        for sref in &wf.attached_signals {
            if let Some(name) = sref.cli_name.as_deref() {
                check(
                    &model.service,
                    &format!(
                        "workflow `{}` signal[ref={}] cli.name",
                        wf.rpc_method, sref.rpc_method
                    ),
                    name,
                )?;
            }
            for alias in &sref.cli_aliases {
                check(
                    &model.service,
                    &format!(
                        "workflow `{}` signal[ref={}] cli.aliases entry",
                        wf.rpc_method, sref.rpc_method
                    ),
                    alias,
                )?;
            }
        }
        // Per-update-ref (WorkflowOptions.update[N].cli).
        for uref in &wf.attached_updates {
            if let Some(name) = uref.cli_name.as_deref() {
                check(
                    &model.service,
                    &format!(
                        "workflow `{}` update[ref={}] cli.name",
                        wf.rpc_method, uref.rpc_method
                    ),
                    name,
                )?;
            }
            for alias in &uref.cli_aliases {
                check(
                    &model.service,
                    &format!(
                        "workflow `{}` update[ref={}] cli.aliases entry",
                        wf.rpc_method, uref.rpc_method
                    ),
                    alias,
                )?;
            }
        }
    }
    // Method-level (temporal.v1.{signal,query,update}).cli.
    for s in &model.signals {
        if let Some(name) = s.cli_name.as_deref() {
            check(
                &model.service,
                &format!("signal `{}` cli.name", s.rpc_method),
                name,
            )?;
        }
        for alias in &s.cli_aliases {
            check(
                &model.service,
                &format!("signal `{}` cli.aliases entry", s.rpc_method),
                alias,
            )?;
        }
    }
    for q in &model.queries {
        if let Some(name) = q.cli_name.as_deref() {
            check(
                &model.service,
                &format!("query `{}` cli.name", q.rpc_method),
                name,
            )?;
        }
        for alias in &q.cli_aliases {
            check(
                &model.service,
                &format!("query `{}` cli.aliases entry", q.rpc_method),
                alias,
            )?;
        }
    }
    for u in &model.updates {
        if let Some(name) = u.cli_name.as_deref() {
            check(
                &model.service,
                &format!("update `{}` cli.name", u.rpc_method),
                name,
            )?;
        }
        for alias in &u.cli_aliases {
            check(
                &model.service,
                &format!("update `{}` cli.aliases entry", u.rpc_method),
                alias,
            )?;
        }
    }
    Ok(())
}

/// Reject any `name:` override that contains whitespace or control
/// characters. Temporal's workflow / signal / query / update / activity
/// names land directly on the wire; whitespace would round-trip
/// successfully but make debugging awful (logs show "MyWorkflow "
/// with an invisible trailing space). Newlines and tabs in a name
/// are almost always a paste accident. Empty names from an explicit
/// `name: ""` override are rejected too — proto3 omits the field
/// when empty, so a literal empty override is an authoring mistake.
fn reject_unprintable_registered_names(model: &ServiceModel) -> Result<()> {
    fn check(model_service: &str, kind: &str, rpc: &str, name: &str) -> Result<()> {
        if name.is_empty() {
            bail!(
                "{model_service}.{rpc}: {kind} registered name is empty — set `name:` to a non-empty value or omit the field to use the default",
            );
        }
        for c in name.chars() {
            if c.is_whitespace() || c.is_control() {
                bail!(
                    "{model_service}.{rpc}: {kind} registered name {name:?} contains whitespace / control character `{c:?}` — Temporal names must be printable ASCII without spaces",
                );
            }
        }
        Ok(())
    }
    for wf in &model.workflows {
        check(
            &model.service,
            "workflow",
            &wf.rpc_method,
            &wf.registered_name,
        )?;
        for alias in &wf.aliases {
            check(&model.service, "workflow alias", &wf.rpc_method, alias)?;
        }
    }
    for s in &model.signals {
        check(&model.service, "signal", &s.rpc_method, &s.registered_name)?;
    }
    for q in &model.queries {
        check(&model.service, "query", &q.rpc_method, &q.registered_name)?;
    }
    for u in &model.updates {
        check(&model.service, "update", &u.rpc_method, &u.registered_name)?;
    }
    for a in &model.activities {
        check(
            &model.service,
            "activity",
            &a.rpc_method,
            &a.registered_name,
        )?;
    }
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
    use heck::{ToKebabCase, ToPascalCase};

    // Each CLI subcommand value maps to its owning workflow. We
    // collect both:
    //   1. Explicit `cli.name` and `cli.aliases` values.
    //   2. Clap's default subcommand value derived from the variant
    //      name, kebab-cased — e.g. `Wf1` becomes `start-wf-1`.
    // A collision between (1) on one workflow and (2) on another
    // is the same duplicate-subcommand bug at runtime.
    let mut owners: HashMap<String, String> = HashMap::new();
    let bail_collision = |value: &str,
                          prior: &str,
                          owner: &str,
                          model_service: &str|
     -> Result<()> {
        bail!(
            "{model_service}: cli subcommand value `{value}` is declared by both `{prior}` and `{owner}` — clap would reject the duplicate at runtime; reconcile the cli.name / cli.aliases values",
        );
    };
    for wf in &model.workflows {
        if wf.cli_ignore {
            continue;
        }
        // Explicit overrides take priority over derived defaults.
        let explicit: Vec<String> = wf
            .cli_name
            .iter()
            .chain(wf.cli_aliases.iter())
            .cloned()
            .collect();
        if !explicit.is_empty() {
            for value in &explicit {
                if let Some(prior) = owners.insert(value.clone(), wf.rpc_method.clone()) {
                    bail_collision(value, &prior, &wf.rpc_method, &model.service)?;
                }
            }
        } else {
            // Default-derived: clap uses the kebab-case of the variant
            // name (Pascal-cased rpc method). Same value clap would
            // emit on the wire.
            let derived = wf.rpc_method.to_pascal_case().to_kebab_case();
            if let Some(prior) = owners.insert(derived.clone(), wf.rpc_method.clone()) {
                bail_collision(&derived, &prior, &wf.rpc_method, &model.service)?;
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
