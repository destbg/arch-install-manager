use arch_install_manager::constants::TIMESHIFT_COMMENT;
use arch_install_manager::helpers::timeshift::{
    get_timeshift_snapshots_to_delete, parse_timeshift_snapshot_list,
};
use arch_install_manager::models::snapshot_retention_period::SnapshotRetentionPeriod;
use chrono::{Duration, Local};

fn name(age: Duration) -> String {
    return (Local::now() - age).format("%Y-%m-%d_%H-%M-%S").to_string();
}

fn snapshot(name: &str) -> (String, Option<String>) {
    return (name.to_string(), Some(TIMESHIFT_COMMENT.to_string()));
}

#[test]
fn forever_still_enforces_count_and_scopes_cleanup_to_this_app() {
    let newest = name(Duration::hours(1));
    let older = name(Duration::days(2));
    let oldest = name(Duration::days(400));
    let snapshots = vec![
        snapshot(&newest),
        (name(Duration::days(3)), Some("another-manager".to_string())),
        snapshot(&oldest),
        (name(Duration::days(4)), None),
        (
            name(Duration::days(5)),
            Some(format!("{TIMESHIFT_COMMENT}-manual")),
        ),
        snapshot(&older),
    ];
    let deleted = get_timeshift_snapshots_to_delete(
        snapshots,
        TIMESHIFT_COMMENT,
        1,
        &SnapshotRetentionPeriod::Forever,
        &newest,
    )
    .unwrap();
    assert_eq!(deleted, vec![oldest, older]);
}

#[test]
fn every_age_limit_deletes_expired_snapshots_even_below_count_limit() {
    for (period, days) in [
        (SnapshotRetentionPeriod::Day, 1),
        (SnapshotRetentionPeriod::Week, 7),
        (SnapshotRetentionPeriod::Month, 30),
        (SnapshotRetentionPeriod::Year, 365),
    ] {
        let newest = name(Duration::hours(1));
        let recent = name(Duration::hours(2));
        let expired = name(Duration::days(days) + Duration::hours(2));
        let deleted = get_timeshift_snapshots_to_delete(
            vec![snapshot(&expired), snapshot(&recent), snapshot(&newest)],
            TIMESHIFT_COMMENT,
            10,
            &period,
            &newest,
        )
        .unwrap();
        assert_eq!(deleted, vec![expired], "{period:?}");
    }
}

#[test]
fn count_and_age_limits_both_apply() {
    let newest = name(Duration::hours(1));
    let second = name(Duration::hours(2));
    let excess = name(Duration::hours(3));
    let expired = name(Duration::days(8));
    let deleted = get_timeshift_snapshots_to_delete(
        vec![
            snapshot(&expired),
            snapshot(&second),
            snapshot(&excess),
            snapshot(&newest),
        ],
        TIMESHIFT_COMMENT,
        2,
        &SnapshotRetentionPeriod::Week,
        &newest,
    )
    .unwrap();
    assert_eq!(deleted, vec![expired, excess]);
}

#[test]
fn protected_snapshot_counts_toward_limit_even_when_older_or_expired() {
    let protected = name(Duration::days(400));
    let newer = name(Duration::hours(1));
    let deleted = get_timeshift_snapshots_to_delete(
        vec![snapshot(&protected), snapshot(&newer)],
        TIMESHIFT_COMMENT,
        1,
        &SnapshotRetentionPeriod::Day,
        &protected,
    )
    .unwrap();
    assert_eq!(deleted, vec![newer]);
}

#[test]
fn invalid_policy_or_missing_protected_snapshot_aborts_cleanup() {
    let newest = name(Duration::hours(1));
    assert!(
        get_timeshift_snapshots_to_delete(
            vec![snapshot(&newest)],
            TIMESHIFT_COMMENT,
            0,
            &SnapshotRetentionPeriod::Forever,
            &newest,
        )
        .is_err()
    );
    assert!(
        get_timeshift_snapshots_to_delete(
            vec![snapshot(&newest)],
            TIMESHIFT_COMMENT,
            1,
            &SnapshotRetentionPeriod::Forever,
            "missing",
        )
        .is_err()
    );
    assert!(
        get_timeshift_snapshots_to_delete(
            vec![snapshot(&newest), snapshot("../../not-a-snapshot")],
            TIMESHIFT_COMMENT,
            1,
            &SnapshotRetentionPeriod::Forever,
            &newest,
        )
        .is_err()
    );
    assert!(
        get_timeshift_snapshots_to_delete(
            vec![(newest.clone(), Some(String::new()))],
            "",
            1,
            &SnapshotRetentionPeriod::Forever,
            &newest,
        )
        .is_err()
    );
}

#[test]
fn parses_timeshift_table_without_crossing_rows_or_losing_multiple_tags() {
    let output = format!(
        "Num  Name                 Tags  Description\n\
         0 > 2026-01-01_12-00-00  O\n\
         1 > 2026-01-02_12-00-00  O W  {TIMESHIFT_COMMENT}\n\
         2   2026-01-03_12-00-00  D    Manual backup\n\
         3 > 2026-01-04_12-00-00  O    O {TIMESHIFT_COMMENT}\n"
    );
    let snapshots = parse_timeshift_snapshot_list(&output).unwrap();
    assert_eq!(
        snapshots,
        vec![
            ("2026-01-01_12-00-00".to_string(), None),
            snapshot("2026-01-02_12-00-00"),
            (
                "2026-01-03_12-00-00".to_string(),
                Some("Manual backup".to_string())
            ),
            (
                "2026-01-04_12-00-00".to_string(),
                Some(format!("O {TIMESHIFT_COMMENT}"))
            ),
        ]
    );
    assert!(parse_timeshift_snapshot_list("unrecognized output").is_err());
}
