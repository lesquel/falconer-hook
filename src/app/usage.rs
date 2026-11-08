//! `usage`: token usage per agent from the agents' local logs.

use std::fmt::Write as _;

use super::Ctx;
use crate::cli::output::{CliError, Reply};
use crate::features::usage::application::ReportUsage;
use crate::features::usage::domain::{AgentUsage, Days, Tokens, UsageReport};
use crate::features::usage::infrastructure::{local_utc_offset, FsUsageLogs};
use crate::shared::infrastructure::system::SystemClock;

pub fn usage(ctx: &Ctx, days: i64) -> Result<Reply, CliError> {
    let days = Days::new(days)?;
    let logs = FsUsageLogs::new(&ctx.paths.home);
    let report = ReportUsage {
        logs: &logs,
        clock: &SystemClock,
        utc_offset: local_utc_offset(&ctx.runner),
    }
    .run(days);
    let human = render(&report);
    Ok(Reply::new(&report, human))
}

/// `1234567` → `1.2M`.
fn short(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.1}B", n as f64 / 1e9),
    }
}

fn breakdown(t: &Tokens) -> String {
    format!(
        "{} (in {} · out {} · cache read {} · cache write {})",
        short(t.total()),
        short(t.input),
        short(t.output),
        short(t.cache_read),
        short(t.cache_write)
    )
}

fn render_agent(out: &mut String, a: &AgentUsage, days: u32, now: u64) {
    if !a.available {
        let _ = writeln!(out, "{:<9} {}", a.agent, a.note.as_deref().unwrap_or("-"));
        return;
    }
    let _ = writeln!(out, "{}", a.agent);
    let _ = writeln!(out, "  today     {}", breakdown(&a.today));
    let _ = writeln!(
        out,
        "  {:<9} {}",
        format!("{days} days"),
        breakdown(&a.period)
    );
    let list = |items: Vec<(String, u64)>| {
        items
            .into_iter()
            .map(|(n, t)| format!("{n} {}", short(t)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !a.projects.is_empty() {
        let p = a
            .projects
            .iter()
            .map(|p| (p.project.clone(), p.tokens.total()));
        let _ = writeln!(out, "  projects  {}", list(p.collect()));
    }
    if !a.models.is_empty() {
        let m = a.models.iter().map(|m| (m.model.clone(), m.tokens.total()));
        let _ = writeln!(out, "  models    {}", list(m.collect()));
    }
    for l in &a.limits {
        let resets = match l.resets_at {
            Some(t) if t > now => {
                let m = (t - now) / 60;
                format!(", resets in {}h{:02}m", m / 60, m % 60)
            }
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "  limit     {} {:.0}% used{resets}",
            l.name, l.used_percent
        );
    }
    if let Some(n) = &a.note {
        let _ = writeln!(out, "  note      {n}");
    }
}

fn render(r: &UsageReport) -> String {
    let mut out = String::new();
    for a in &r.agents {
        render_agent(&mut out, a, r.days, r.generated_at);
    }
    out.trim_end().to_string()
}
