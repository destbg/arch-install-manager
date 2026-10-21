use std::process::Command;

use anyhow::{Context, Result, anyhow, ensure};
use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone};
use regex::Regex;

use crate::log_info;
use crate::models::snapshot_retention_period::SnapshotRetentionPeriod;

pub fn create_timeshift_snapshot(comment: &str) -> Result<String> {
    let status = Command::new("timeshift")
        .args([
            "--create",
            "--tags",
            "O",
            "--comments",
            comment,
            "--yes",
            "--scripted",
        ])
        .status()?;
    ensure!(status.success(), "timeshift snapshot create failed");

    return list_timeshift_snapshots_with_comments()?
        .into_iter()
        .filter(|(_, description)| description.as_deref() == Some(comment))
        .map(|(name, _)| name)
        .max()
        .ok_or_else(|| anyhow!("created snapshot not found in list"));
}

pub fn cleanup_timeshift_snapshots(
    comment: &str,
    retention_count: u32,
    retention_period: &SnapshotRetentionPeriod,
    keep_snapshot: &str,
) -> Result<()> {
    let snapshots = list_timeshift_snapshots_with_comments()?;
    let to_delete = get_timeshift_snapshots_to_delete(
        snapshots,
        comment,
        retention_count,
        retention_period,
        keep_snapshot,
    )?;
    for name in to_delete {
        let status = Command::new("timeshift")
            .args(["--delete", "--snapshot", &name, "--yes", "--scripted"])
            .status()?;
        ensure!(status.success(), "failed to delete snapshot {name}");
        log_info!("Deleted snapshot {name}");
    }
    return Ok(());
}

pub fn get_timeshift_snapshots_to_delete(
    snapshots: Vec<(String, Option<String>)>,
    comment: &str,
    retention_count: u32,
    retention_period: &SnapshotRetentionPeriod,
    keep_snapshot: &str,
) -> Result<Vec<String>> {
    ensure!(
        retention_count > 0,
        "At least one snapshot must be retained"
    );
    ensure!(
        !comment.trim().is_empty(),
        "Snapshot description must not be empty"
    );
    let mut names: Vec<_> = snapshots
        .into_iter()
        .filter(|(_, description)| description.as_deref().map(str::trim) == Some(comment))
        .map(|(name, _)| name.trim().to_string())
        .collect();
    ensure!(
        names.iter().any(|name| name == keep_snapshot),
        "Protected snapshot not found; cleanup aborted"
    );
    names.sort_by(|a, b| b.cmp(a));
    names.dedup();

    let now = Local::now();
    let cutoff = match retention_period {
        SnapshotRetentionPeriod::Forever => None,
        SnapshotRetentionPeriod::Day => Some(now - Duration::days(1)),
        SnapshotRetentionPeriod::Week => Some(now - Duration::weeks(1)),
        SnapshotRetentionPeriod::Month => Some(now - Duration::days(30)),
        SnapshotRetentionPeriod::Year => Some(now - Duration::days(365)),
    };
    let mut kept = 1;
    let mut to_delete = Vec::new();
    for name in names {
        if name == keep_snapshot {
            continue;
        }
        let timestamp = parse_snapshot_timestamp(&name)?;
        if kept >= retention_count || cutoff.is_some_and(|cutoff| timestamp < cutoff) {
            to_delete.push(name);
        } else {
            kept += 1;
        }
    }
    to_delete.reverse();
    return Ok(to_delete);
}

pub fn parse_timeshift_snapshot_list(output: &str) -> Result<Vec<(String, Option<String>)>> {
    let table = Regex::new(
        r"(?m)^[ \t]*\d+[ \t]+(?:>[ \t]+)?([0-9]{4}-[0-9]{2}-[0-9]{2}_[0-9]{2}-[0-9]{2}-[0-9]{2})[ \t]+[OBHDWM](?: [OBHDWM])*(?:[ \t]{2,}(.*\S))?[ \t]*$",
    )?;
    let snapshots: Vec<_> = table
        .captures_iter(output)
        .map(|capture| {
            return (
                capture[1].to_string(),
                capture
                    .get(2)
                    .map(|description| description.as_str().trim().to_string()),
            );
        })
        .collect();
    ensure!(
        !snapshots.is_empty(),
        "Could not parse Timeshift snapshot list"
    );
    return Ok(snapshots);
}

fn list_timeshift_snapshots_with_comments() -> Result<Vec<(String, Option<String>)>> {
    let output = Command::new("timeshift")
        .env("LC_ALL", "C")
        .args(["--list"])
        .output()
        .context("timeshift --list")?;
    ensure!(output.status.success(), "timeshift --list failed");
    return parse_timeshift_snapshot_list(&String::from_utf8_lossy(&output.stdout));
}

fn parse_snapshot_timestamp(name: &str) -> Result<DateTime<Local>> {
    let timestamp = NaiveDateTime::parse_from_str(name, "%Y-%m-%d_%H-%M-%S")
        .with_context(|| format!("Invalid snapshot timestamp: {name}"))?;
    ensure!(
        timestamp.format("%Y-%m-%d_%H-%M-%S").to_string() == name,
        "Invalid snapshot name: {name}"
    );
    return Local
        .from_local_datetime(&timestamp)
        .earliest()
        .ok_or_else(|| anyhow!("Invalid local snapshot timestamp: {name}"));
}
