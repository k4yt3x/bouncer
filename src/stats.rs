use std::fmt::Write;

use crate::error::Result;
use crate::storage::{GroupStats, Storage};

pub async fn render(
    storage: &Storage,
    only_group: Option<i64>,
    since: Option<i64>,
) -> Result<String> {
    let mut out = String::new();

    if only_group.is_none() {
        let global = storage.stats_global(since).await?;
        writeln!(out, "Global").unwrap();
        write_stats(&mut out, &global);
        writeln!(out).unwrap();
    }

    let per_group = storage.stats_by_group(since).await?;
    let scoped: Vec<(i64, GroupStats)> = match only_group {
        Some(id) => per_group
            .into_iter()
            .filter(|(cid, _)| *cid == id)
            .collect(),
        None => per_group,
    };

    if scoped.is_empty() {
        writeln!(out, "No verification records in range.").unwrap();
        return Ok(out);
    }

    for (chat_id, stats) in scoped {
        writeln!(out, "Group {chat_id}").unwrap();
        write_stats(&mut out, &stats);
        writeln!(out).unwrap();
    }

    Ok(out)
}

fn write_stats(out: &mut String, stats: &GroupStats) {
    let attempts = stats.attempts;
    let approved = stats.approved;
    let rejected = stats.rejected();
    let approval_pct = if attempts == 0 {
        0.0
    } else {
        (approved as f64 / attempts as f64) * 100.0
    };

    writeln!(out, "  attempts:   {attempts}").unwrap();
    writeln!(out, "  approved:   {approved} ({approval_pct:.1}%)").unwrap();
    writeln!(out, "  rejected:   {rejected}").unwrap();
    writeln!(out, "    wrong_answer: {}", stats.rejected_wrong).unwrap();
    writeln!(out, "    no_button:    {}", stats.rejected_no_button).unwrap();
    writeln!(out, "    no_answer:    {}", stats.rejected_no_answer).unwrap();
    writeln!(out, "    llm_error:    {}", stats.rejected_llm_error).unwrap();
    writeln!(out, "    cooldown:     {}", stats.rejected_cooldown).unwrap();
    writeln!(out, "  unique users: {}", stats.unique_users).unwrap();
}
