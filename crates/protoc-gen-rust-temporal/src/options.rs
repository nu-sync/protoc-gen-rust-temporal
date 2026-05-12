//! Plugin invocation options, parsed once from the protoc/buf-supplied
//! parameter string and threaded through `run_with_pool`.
//!
//! Strict by design: any unknown key is rejected so that typos like
//! `opt: [worker=true]` (missing `s`) fail loudly instead of silently
//! emitting nothing. See the cludden-parity reframe design doc for the
//! full surface; Phase 2 wires only `activities`.

use anyhow::{Result, anyhow};

#[derive(Debug, Default, Clone, Copy)]
pub struct RenderOptions {
    /// Emit the per-service `<Service>Activities` async trait + per-activity
    /// name consts when the service has activity-annotated methods.
    pub activities: bool,
    /// Emit per-rpc signal/query/update name consts at module level so the
    /// consumer's hand-rolled `#[workflow]` setup can reference them instead
    /// of string literals. Phase 3.0 (Option C from the spike findings) ships
    /// name consts only; the workflow trait emit is deferred to Phase 3.1
    /// pending an adapter prototype against the SDK's `#[workflow]` macro.
    pub workflows: bool,
    /// Emit a per-service `<service>_cli` module with clap-derive `Cli` +
    /// per-workflow `Start<Workflow>` / `Attach<Workflow>` subcommands.
    /// Phase 4.0 ships the parser structure only; the `Cli::run` dispatch
    /// impl is deferred to Phase 4.1 once the JSON-input → proto deserialize
    /// path is decided.
    pub cli: bool,
}

/// Parse the protoc plugin parameter string.
///
/// Grammar: `key=val,key=val,...`. Whitespace trimmed around keys and values.
/// Empty input yields the default (all flags `false`).
pub fn parse_options(s: &str) -> Result<RenderOptions> {
    let mut out = RenderOptions::default();
    for pair in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| anyhow!("plugin option `{pair}` missing `=value`"))?;
        let key = key.trim();
        let value = value.trim();
        match key {
            "activities" => out.activities = parse_bool(key, value)?,
            "workflows" => out.workflows = parse_bool(key, value)?,
            "cli" => out.cli = parse_bool(key, value)?,
            other => return Err(anyhow!("unknown plugin option `{other}`")),
        }
    }
    Ok(out)
}

fn parse_bool(key: &str, value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(anyhow!(
            "plugin option `{key}` expects `true|false`, got `{other}`"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        let opts = parse_options("").unwrap();
        assert!(!opts.activities);
    }

    #[test]
    fn comma_separated_pairs() {
        let opts = parse_options("activities=true").unwrap();
        assert!(opts.activities);
    }

    #[test]
    fn whitespace_tolerated() {
        let opts = parse_options(" activities = true ").unwrap();
        assert!(opts.activities);
    }

    #[test]
    fn rejects_unknown_keys() {
        let err = parse_options("activitie=true").unwrap_err();
        assert!(err.to_string().contains("activitie"), "{err}");
    }

    #[test]
    fn rejects_bad_bool() {
        let err = parse_options("activities=yes").unwrap_err();
        assert!(err.to_string().contains("activities"), "{err}");
    }

    #[test]
    fn rejects_missing_value() {
        let err = parse_options("activities").unwrap_err();
        assert!(err.to_string().contains("activities"), "{err}");
    }
}
