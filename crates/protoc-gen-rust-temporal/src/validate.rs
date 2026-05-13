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
    validate_workflows(model)?;
    validate_signal_outputs(model)?;
    validate_empty_with_start(model)?;
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
