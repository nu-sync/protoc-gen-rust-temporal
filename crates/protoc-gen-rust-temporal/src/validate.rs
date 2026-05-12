//! Cross-method invariants applied after `parse.rs` builds a `ServiceModel`.
//!
//! Errors here translate directly into `CodeGeneratorResponse.error` and
//! surface to the user as `protoc` failures, so messages should pinpoint
//! the service + rpc + offending option.

use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};

use crate::model::ServiceModel;

pub fn validate(model: &ServiceModel) -> Result<()> {
    reject_rpc_collisions(model)?;
    validate_workflows(model)?;
    validate_signal_outputs(model)?;
    Ok(())
}

/// A single rpc method may carry at most one `temporal.v1.*` annotation;
/// declaring two on the same rpc collapses to a single entry in `parse.rs`
/// (first match wins), but two different annotation buckets pointing at the
/// same method *name* — which can happen when an activity is named the same
/// as a sibling workflow — would break the generated handle. Reject up front.
fn reject_rpc_collisions(model: &ServiceModel) -> Result<()> {
    let mut seen: HashMap<&str, &'static str> = HashMap::new();

    let kinds: [(&'static str, Vec<&str>); 5] = [
        (
            "workflow",
            model
                .workflows
                .iter()
                .map(|w| w.rpc_method.as_str())
                .collect(),
        ),
        (
            "signal",
            model
                .signals
                .iter()
                .map(|s| s.rpc_method.as_str())
                .collect(),
        ),
        (
            "query",
            model
                .queries
                .iter()
                .map(|q| q.rpc_method.as_str())
                .collect(),
        ),
        (
            "update",
            model
                .updates
                .iter()
                .map(|u| u.rpc_method.as_str())
                .collect(),
        ),
        (
            "activity",
            model
                .activities
                .iter()
                .map(|a| a.rpc_method.as_str())
                .collect(),
        ),
    ];

    for (kind, names) in &kinds {
        for name in names {
            if let Some(prev) = seen.insert(name, kind) {
                bail!(
                    "{}.{name}: rpc carries conflicting Temporal annotations ({prev} and {kind}) — pick one",
                    model.service,
                );
            }
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
    bail!(
        "{}.{}: workflow references {kind} \"{target}\" but no sibling rpc carries {expected_annotation}",
        model.service,
        wf.rpc_method,
    );
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
