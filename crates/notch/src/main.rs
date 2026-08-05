//! `notch` — turn the HUD, and its individual modules, on or off from a terminal.
//!
//! This only writes the settings file. A running `notch-app` re-reads it on its
//! refresh tick, so a change lands within a few seconds without a restart; if the
//! app isn't running, the setting applies next time it starts.

use anyhow::Result;
use clap::{Parser, Subcommand};
use notch_core::config::{self, NotchConfig, MAX_OPEN_DELAY_MS, MODULES};
use notch_core::doctor::{self, State};

#[derive(Parser)]
#[command(
    name = "notch",
    about = "A Dynamic Island for the Mac notch",
    version,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Show the HUD.
    On,
    /// Hide the HUD without quitting the app.
    Off,
    /// Flip the HUD.
    Toggle,
    /// How long the cursor must rest on the notch before the HUD opens.
    ///
    /// 0 opens the moment the cursor touches it; higher values ignore a cursor
    /// merely passing across the notch.
    Delay {
        /// Milliseconds (0–5000).
        ms: u64,
    },
    /// Turn one module on or off.
    Module {
        /// Module name — see `notch` with no arguments for the list.
        name: String,
        /// on | off | toggle (default: toggle).
        #[arg(default_value = "toggle")]
        state: String,
    },
    /// Hold the panel open, or let it close on the cursor again.
    Pin {
        /// on | off | toggle (default: toggle).
        #[arg(default_value = "toggle")]
        state: String,
    },
    /// Check that everything the HUD depends on is actually wired up.
    ///
    /// Start here when the panel is showing less than you expected: each module
    /// needs something outside this app, and they fail quietly and separately.
    Doctor {
        /// Print JSON instead, and exit non-zero on a real failure.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    run(Cli::parse().action.as_ref(), &config::load())
}

/// Applies one action to `cfg`, printing what happened. Split from [`main`] so
/// the settings are an argument rather than a read.
fn run(action: Option<&Action>, cfg: &NotchConfig) -> Result<()> {
    match action {
        None => {
            print_status(cfg);
            Ok(())
        }
        Some(Action::On) => set(cfg, "enabled", true),
        Some(Action::Off) => set(cfg, "enabled", false),
        Some(Action::Toggle) => set(cfg, "enabled", !cfg.enabled),
        Some(Action::Delay { ms }) => set_delay(cfg, *ms),
        Some(Action::Module { name, state }) => set_module(cfg, name, state),
        Some(Action::Pin { state }) => set_pin(cfg, state),
        Some(Action::Doctor { json }) => run_doctor(*json),
    }
}

/// Holds the panel open, or lets it close again. A running app adopts this on its
/// next reconcile tick, so it lands within a few seconds.
fn set_pin(cfg: &NotchConfig, state: &str) -> Result<()> {
    let want = match state.to_ascii_lowercase().as_str() {
        "on" => true,
        "off" => false,
        "toggle" => !cfg.pinned,
        other => anyhow::bail!("expected on, off, or toggle — got '{other}'"),
    };
    config::save(&cfg.with("pinned", want)?)?;
    println!("panel {}", if want { "pinned open" } else { "unpinned" });
    Ok(())
}

/// Prints every check, and exits non-zero on a real failure so a script can gate
/// on it.
fn run_doctor(as_json: bool) -> Result<()> {
    let checks = doctor::run();

    if as_json {
        println!("{}", serde_json::to_string_pretty(&checks)?);
    } else {
        for c in &checks {
            let mark = match c.state {
                State::Ok => "✓",
                State::Fail => "✗",
                State::Skipped => "–",
            };
            println!("{mark} {:<9} {:<26} {}", c.module, c.name, c.detail);
            if let Some(fix) = &c.fix {
                println!("               → {fix}");
            }
        }
        println!();
        println!(
            "{}",
            if doctor::healthy(&checks) {
                "everything the switched-on modules need is wired up."
            } else {
                "something a switched-on module needs is missing — see the arrows above."
            }
        );
    }

    if doctor::healthy(&checks) {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn set_delay(cfg: &NotchConfig, ms: u64) -> Result<()> {
    let next = cfg.with_open_delay(ms);
    config::save(&next)?;
    if next.open_delay_ms != ms {
        println!(
            "clamped to {}ms (max {MAX_OPEN_DELAY_MS})",
            next.open_delay_ms
        );
    }
    println!("opens after {}", describe_delay(next.open_delay_ms));
    Ok(())
}

fn set_module(cfg: &NotchConfig, name: &str, state: &str) -> Result<()> {
    let key = name.to_ascii_lowercase();
    if !MODULES.iter().any(|(m, _)| *m == key) {
        let names: Vec<&str> = MODULES.iter().map(|(m, _)| *m).collect();
        anyhow::bail!("unknown module '{name}' (try: {})", names.join(", "));
    }
    let want = match state.to_ascii_lowercase().as_str() {
        "on" => true,
        "off" => false,
        "toggle" => !cfg.get(&key)?,
        other => anyhow::bail!("expected on, off, or toggle — got '{other}'"),
    };
    set(cfg, &key, want)
}

/// Writes one switch, saying so only if it actually changed.
fn set(cfg: &NotchConfig, key: &str, want: bool) -> Result<()> {
    let label = if key == "enabled" { "notch HUD" } else { key };
    if cfg.get(key)? == want {
        println!("{label} already {}", on_off(want));
        return Ok(());
    }
    config::save(&cfg.with(key, want)?)?;
    println!("{label} {}", on_off(want));
    Ok(())
}

fn print_status(cfg: &NotchConfig) {
    println!("notch HUD  {}", on_off(cfg.enabled));
    println!();
    for (key, desc) in MODULES {
        println!(
            "  {key:<9} {:<4} {desc}",
            on_off(cfg.get(key).unwrap_or(false))
        );
    }
    println!();
    println!("  opens after {}", describe_delay(cfg.open_delay_ms));
    println!();
    println!("notch on | off | toggle");
    println!("notch delay <ms>");
    println!("notch module <name> [on|off|toggle]");
    println!("notch pin [on|off|toggle]");
    println!("notch doctor");
    println!();
    println!("Click the sliver to pin the panel open, or use `notch pin`.");
}

fn describe_delay(ms: u64) -> String {
    if ms == 0 {
        "no delay".to_string()
    } else {
        format!("{ms}ms of hover")
    }
}

fn on_off(v: bool) -> &'static str {
    if v {
        "on"
    } else {
        "off"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_cli_definition_is_valid() {
        // Catches conflicting flags, duplicate names and bad defaults, which clap
        // only asserts at runtime.
        Cli::command().debug_assert();
    }

    #[test]
    fn delay_is_described_in_words() {
        assert_eq!(describe_delay(0), "no delay");
        assert_eq!(describe_delay(600), "600ms of hover");
    }

    #[test]
    fn on_off_reads_as_a_switch() {
        assert_eq!(on_off(true), "on");
        assert_eq!(on_off(false), "off");
    }

    #[test]
    fn status_needs_no_config_file() {
        // Printing the status must never be the thing that fails.
        print_status(&NotchConfig::default());
    }

    #[test]
    fn an_unknown_module_is_rejected_before_anything_is_written() {
        let err = set_module(&NotchConfig::default(), "solat", "on").unwrap_err();
        assert!(err.to_string().contains("unknown module"), "{err}");
        assert!(err.to_string().contains("day"), "it suggests what is valid");
    }

    #[test]
    fn an_unknown_state_is_rejected() {
        let err = set_module(&NotchConfig::default(), "day", "maybe").unwrap_err();
        assert!(
            err.to_string().contains("expected on, off, or toggle"),
            "{err}"
        );
    }

    #[test]
    fn a_no_op_write_is_recognised() {
        // `day` is on by default, so turning it on again must not error.
        assert!(set(&NotchConfig::default(), "day", true).is_ok());
    }

    #[test]
    fn parses_every_documented_invocation() {
        for args in [
            vec!["notch"],
            vec!["notch", "on"],
            vec!["notch", "off"],
            vec!["notch", "toggle"],
            vec!["notch", "delay", "0"],
            vec!["notch", "delay", "600"],
            vec!["notch", "module", "day"],
            vec!["notch", "module", "day", "off"],
            vec!["notch", "pin"],
            vec!["notch", "pin", "on"],
            vec!["notch", "doctor"],
            vec!["notch", "doctor", "--json"],
        ] {
            assert!(
                Cli::try_parse_from(&args).is_ok(),
                "failed to parse {args:?}"
            );
        }
    }

    #[test]
    fn module_state_defaults_to_toggle() {
        let cli = Cli::try_parse_from(["notch", "module", "day"]).unwrap();
        match cli.action {
            Some(Action::Module { state, .. }) => assert_eq!(state, "toggle"),
            _ => panic!("expected a module action"),
        }
    }
}
