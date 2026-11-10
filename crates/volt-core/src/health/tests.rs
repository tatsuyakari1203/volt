use std::collections::HashSet;

use super::*;
use crate::error::PortError;
use crate::fakes::FakeRegistry;
use crate::metrics::fake::{FakeMetrics, typical_sample};
use crate::ports::MemoryStatus;
use crate::registry::{RegistryHive, RegistryValue, RegistryValueRef, RegistryView};

const GB: u64 = GIB;

fn disk(total: u64, free: u64) -> DiskSpace {
    DiskSpace {
        mount: "C:\\".into(),
        total_bytes: total,
        free_bytes: free,
    }
}

/// Inputs where every check ran and found nothing.
fn healthy() -> HealthInputs {
    HealthInputs {
        system_disk: Some(disk(500 * GB, 200 * GB)),
        memory_used_percent: Some(40.0),
        restart_pending: Some(false),
        startup_load: None,
    }
}

fn ids(report: &HealthReport) -> Vec<&'static str> {
    report.deductions().iter().map(|d| d.id).collect()
}

fn only_deduction(report: &HealthReport) -> &Deduction {
    assert_eq!(report.deductions().len(), 1, "{:?}", report.deductions());
    &report.deductions()[0]
}

// --- Rules ------------------------------------------------------------------------------------

#[test]
fn a_healthy_machine_scores_100_with_no_deductions() {
    let report = assess(&healthy());
    assert_eq!(report.score(), 100);
    assert!(report.deductions().is_empty());
    assert!(report.unchecked().is_empty());
    assert_eq!(report.level(), HealthLevel::Good);
}

#[test]
fn disk_tiers_follow_the_free_share_of_a_small_disk() {
    // A 200 GB disk: free space between 5 GB and 100 GB is judged by its share.
    let total = 200 * GB;
    let cases = [
        // 6 GB free is 3 percent: critical.
        (6 * GB, Some(("disk.critical", DISK_CRITICAL_POINTS))),
        // Exactly 5 percent is not below 5 percent: low.
        (10 * GB, Some(("disk.low", DISK_LOW_POINTS))),
        // 9.9 percent: low.
        (19 * GB + 800 * MIB, Some(("disk.low", DISK_LOW_POINTS))),
        // Exactly 10 percent: watch.
        (20 * GB, Some(("disk.watch", DISK_WATCH_POINTS))),
        // 14.9 percent: watch.
        (29 * GB + 800 * MIB, Some(("disk.watch", DISK_WATCH_POINTS))),
        // Exactly 15 percent: fine.
        (30 * GB, None),
        (150 * GB, None),
    ];
    for (free, expected) in cases {
        let report = assess(&HealthInputs {
            system_disk: Some(disk(total, free)),
            ..healthy()
        });
        let found = report.deductions().first().map(|d| (d.id, d.points));
        assert_eq!(found, expected, "free = {free}");
        assert!(report.deductions().len() <= 1);
    }
}

#[test]
fn under_5_gb_free_is_critical_on_any_disk() {
    // 4.9 GB is 49 percent of a 10 GB disk, and still critical.
    for (total, free) in [(10 * GB, 4 * GB + 900 * MIB), (2000 * GB, GB), (5 * GB, 0)] {
        let report = assess(&HealthInputs {
            system_disk: Some(disk(total, free)),
            ..healthy()
        });
        assert_eq!(
            only_deduction(&report).id,
            "disk.critical",
            "{total} {free}"
        );
    }
    // Exactly 5 GB on a small disk is judged by share again: 5 of 10 GB is plenty.
    let report = assess(&HealthInputs {
        system_disk: Some(disk(10 * GB, 5 * GB)),
        ..healthy()
    });
    assert!(report.deductions().is_empty());
}

#[test]
fn a_disk_with_100_gb_free_is_never_penalised() {
    // 100 GB of 4 TB is 2.4 percent, and still roomy.
    let report = assess(&HealthInputs {
        system_disk: Some(disk(4000 * GB, 100 * GB)),
        ..healthy()
    });
    assert!(report.deductions().is_empty());
    // One byte less is judged by share: 2.4 percent is critical.
    let report = assess(&HealthInputs {
        system_disk: Some(disk(4000 * GB, 100 * GB - 1)),
        ..healthy()
    });
    assert_eq!(only_deduction(&report).id, "disk.critical");
}

#[test]
fn a_disk_deduction_names_the_drive_the_space_and_points_to_the_cleaner() {
    let report = assess(&HealthInputs {
        system_disk: Some(disk(476 * GB, 24 * GB)),
        ..healthy()
    });
    let deduction = only_deduction(&report);
    assert_eq!(deduction.id, "disk.low");
    assert_eq!(deduction.check, HealthCheck::SystemDisk);
    assert_eq!(deduction.fix, FixAction::OpenCleaner);
    assert_eq!(deduction.points, 20);
    assert_eq!(
        deduction.reason,
        "24.0 GB of 476 GB is free on C: (5.0%). Windows needs room to install updates and to \
         move memory to disk."
    );
    assert_eq!(report.score(), 80);
}

#[test]
fn a_disk_with_no_capacity_or_more_free_than_capacity_is_not_a_finding() {
    for system_disk in [disk(0, 0), disk(0, 5 * GB), disk(500 * GB, 900 * GB)] {
        let report = assess(&HealthInputs {
            system_disk: Some(system_disk),
            ..healthy()
        });
        assert!(report.deductions().is_empty());
    }
}

#[test]
fn memory_tiers_use_inclusive_thresholds() {
    let cases = [
        (0.0, None),
        (79.9, None),
        (80.0, Some(("memory.high", MEMORY_HIGH_POINTS))),
        (89.9, Some(("memory.high", MEMORY_HIGH_POINTS))),
        (90.0, Some(("memory.critical", MEMORY_CRITICAL_POINTS))),
        (100.0, Some(("memory.critical", MEMORY_CRITICAL_POINTS))),
    ];
    for (used, expected) in cases {
        let report = assess(&HealthInputs {
            memory_used_percent: Some(used),
            ..healthy()
        });
        let found = report.deductions().first().map(|d| (d.id, d.points));
        assert_eq!(found, expected, "used = {used}");
    }
}

#[test]
fn a_memory_deduction_says_the_share_and_points_to_memory() {
    let report = assess(&HealthInputs {
        memory_used_percent: Some(87.4),
        ..healthy()
    });
    let deduction = only_deduction(&report);
    assert_eq!(deduction.check, HealthCheck::Memory);
    assert_eq!(deduction.fix, FixAction::OpenMemory);
    assert!(
        deduction
            .reason
            .starts_with("87% of your memory is in use.")
    );
}

#[test]
fn a_pending_restart_takes_10_points_and_says_to_restart() {
    let report = assess(&HealthInputs {
        restart_pending: Some(true),
        ..healthy()
    });
    let deduction = only_deduction(&report);
    assert_eq!(deduction.id, "restart.pending");
    assert_eq!(deduction.points, 10);
    assert_eq!(deduction.fix, FixAction::RestartWindows);
    assert_eq!(report.score(), 90);
}

#[test]
fn startup_load_is_judged_only_when_it_is_known() {
    // Not known yet: no deduction, and not listed as unchecked either.
    let report = assess(&healthy());
    assert!(report.deductions().is_empty());
    assert!(!report.unchecked().contains(&HealthCheck::Startup));

    let cases = [
        (0, None),
        (1, None),
        (2, Some(("startup.some", STARTUP_SOME_POINTS))),
        (3, Some(("startup.some", STARTUP_SOME_POINTS))),
        (4, Some(("startup.many", STARTUP_MANY_POINTS))),
        (30, Some(("startup.many", STARTUP_MANY_POINTS))),
    ];
    for (apps, expected) in cases {
        let report = assess(&HealthInputs {
            startup_load: Some(StartupLoad {
                high_impact_apps: apps,
            }),
            ..healthy()
        });
        let found = report.deductions().first().map(|d| (d.id, d.points));
        assert_eq!(found, expected, "apps = {apps}");
        if let Some(deduction) = report.deductions().first() {
            assert_eq!(deduction.fix, FixAction::OpenStartup);
            assert!(deduction.reason.starts_with(&format!("{apps} apps")));
        }
    }
}

#[test]
fn deductions_are_listed_in_rule_order_and_add_up() {
    let report = assess(&HealthInputs {
        system_disk: Some(disk(100 * GB, 2 * GB)),
        memory_used_percent: Some(95.0),
        restart_pending: Some(true),
        startup_load: Some(StartupLoad {
            high_impact_apps: 9,
        }),
    });
    assert_eq!(
        ids(&report),
        [
            "disk.critical",
            "memory.critical",
            "restart.pending",
            "startup.many"
        ]
    );
    assert_eq!(report.score(), 100 - MAX_TOTAL_DEDUCTION);
    assert_eq!(report.level(), HealthLevel::Poor);
}

// --- Inputs that could not be read ------------------------------------------------------------

#[test]
fn an_input_that_could_not_be_read_is_listed_and_costs_nothing() {
    let report = assess(&HealthInputs {
        system_disk: None,
        memory_used_percent: Some(40.0),
        restart_pending: None,
        startup_load: None,
    });
    assert_eq!(report.score(), 100);
    assert_eq!(
        report.unchecked(),
        [HealthCheck::SystemDisk, HealthCheck::PendingRestart]
    );
    // Something ran, so there is still a grade.
    assert_eq!(report.level(), HealthLevel::Good);
}

#[test]
fn a_non_finite_memory_figure_counts_as_unchecked() {
    for value in [f32::NAN, f32::INFINITY] {
        let report = assess(&HealthInputs {
            memory_used_percent: Some(value),
            ..healthy()
        });
        assert!(report.deductions().is_empty());
        assert_eq!(report.unchecked(), [HealthCheck::Memory]);
    }
}

#[test]
fn when_nothing_could_be_checked_there_is_no_grade() {
    let report = assess(&HealthInputs::default());
    assert_eq!(report.level(), HealthLevel::Unknown);
    assert_eq!(
        report.unchecked(),
        [
            HealthCheck::SystemDisk,
            HealthCheck::Memory,
            HealthCheck::PendingRestart
        ]
    );
}

#[test]
fn levels_change_at_the_named_scores() {
    let level = |inputs: HealthInputs| {
        let report = assess(&inputs);
        (report.score(), report.level())
    };
    let low_disk = Some(disk(200 * GB, 15 * GB)); // 7.5 percent: 20 points
    let critical_disk = Some(disk(200 * GB, 6 * GB)); // 3 percent: 30 points
    assert_eq!(level(healthy()), (100, HealthLevel::Good));
    // 80 is the lowest Good score.
    assert_eq!(
        level(HealthInputs {
            system_disk: low_disk.clone(),
            ..healthy()
        }),
        (80, HealthLevel::Good)
    );
    // 78 is Fair.
    assert_eq!(
        level(HealthInputs {
            memory_used_percent: Some(85.0),
            restart_pending: Some(true),
            ..healthy()
        }),
        (78, HealthLevel::Fair)
    );
    // 60 is the lowest Fair score.
    assert_eq!(
        level(HealthInputs {
            system_disk: critical_disk.clone(),
            restart_pending: Some(true),
            ..healthy()
        }),
        (60, HealthLevel::Fair)
    );
    // 58 is Poor.
    assert_eq!(
        level(HealthInputs {
            system_disk: critical_disk,
            memory_used_percent: Some(85.0),
            ..healthy()
        }),
        (58, HealthLevel::Poor)
    );
}

// --- The score is exactly 100 minus the listed deductions -------------------------------------

/// A spread of inputs that crosses every threshold of every rule, from both sides, and the
/// missing-input cases.
fn grid() -> Vec<HealthInputs> {
    let disks: Vec<Option<DiskSpace>> = {
        let mut all = vec![None];
        for total in [0, 2 * GB, 10 * GB, 64 * GB, 200 * GB, 476 * GB, 4000 * GB] {
            for free in [
                0,
                1,
                GB,
                5 * GB - 1,
                5 * GB,
                total / 100,
                (total / 20).saturating_sub(1),
                total / 20,
                (total / 10).saturating_sub(1),
                total / 10,
                total / 7,
                total / 5,
                total / 2,
                100 * GB - 1,
                100 * GB,
                total,
                total + GB,
            ] {
                all.push(Some(disk(total, free)));
            }
        }
        all
    };
    let memory = [
        None,
        Some(0.0),
        Some(50.0),
        Some(79.99),
        Some(80.0),
        Some(89.99),
        Some(90.0),
        Some(100.0),
        Some(f32::NAN),
    ];
    let restart = [None, Some(false), Some(true)];
    let startup = [
        None,
        Some(StartupLoad {
            high_impact_apps: 0,
        }),
        Some(StartupLoad {
            high_impact_apps: 1,
        }),
        Some(StartupLoad {
            high_impact_apps: 2,
        }),
        Some(StartupLoad {
            high_impact_apps: 3,
        }),
        Some(StartupLoad {
            high_impact_apps: 4,
        }),
        Some(StartupLoad {
            high_impact_apps: u32::MAX,
        }),
    ];
    let mut inputs = Vec::new();
    for system_disk in &disks {
        for memory_used_percent in memory {
            for restart_pending in restart {
                for startup_load in startup {
                    inputs.push(HealthInputs {
                        system_disk: system_disk.clone(),
                        memory_used_percent,
                        restart_pending,
                        startup_load,
                    });
                }
            }
        }
    }
    inputs
}

#[test]
fn the_score_is_always_100_minus_the_listed_points() {
    let inputs = grid();
    assert!(inputs.len() > 10_000, "the grid is {} inputs", inputs.len());
    let known: HashSet<&str> = DEDUCTION_IDS.into_iter().collect();
    for input in &inputs {
        let report = assess(input);
        let listed: u32 = report
            .deductions()
            .iter()
            .map(|d| u32::from(d.points))
            .sum();

        // The property: nothing but the listed deductions moves the score.
        assert_eq!(u32::from(report.score()), 100 - listed, "{input:?}");
        assert!(listed <= u32::from(MAX_TOTAL_DEDUCTION), "{input:?}");
        assert!(report.score() >= SCORE_MAX - MAX_TOTAL_DEDUCTION);

        // Every deduction is visible and explained.
        let mut seen = HashSet::new();
        for deduction in report.deductions() {
            assert!(known.contains(deduction.id), "{}", deduction.id);
            assert!(seen.insert(deduction.id), "{} listed twice", deduction.id);
            assert!(deduction.points > 0, "{}", deduction.id);
            assert!(!deduction.title.is_empty());
            assert!(!deduction.reason.is_empty());
        }
        // A score below 100 always has at least one deduction, and 100 has none.
        assert_eq!(report.score() == 100, report.deductions().is_empty());

        // Pure: the same input gives the same report.
        assert_eq!(assess(input), report);
    }
}

#[test]
fn the_ids_and_points_of_a_deduction_belong_together() {
    // Each id has one fixed number of points, so an id tells the user exactly what it cost.
    let mut points_of = std::collections::HashMap::new();
    for input in grid() {
        for deduction in assess(&input).deductions() {
            let previous = points_of.insert(deduction.id, deduction.points);
            assert!(
                previous.is_none_or(|p| p == deduction.points),
                "{}",
                deduction.id
            );
        }
    }
    // Every id the rules can produce occurs somewhere in the grid.
    for id in DEDUCTION_IDS {
        assert!(points_of.contains_key(id), "{id} never produced");
    }
}

#[test]
fn less_free_space_never_deducts_fewer_points() {
    for total in [2 * GB, 64 * GB, 476 * GB, 4000 * GB] {
        let mut previous = u8::MAX;
        let step = (total / 400).max(1);
        let mut free = 0;
        while free <= total {
            let points = assess(&HealthInputs {
                system_disk: Some(disk(total, free)),
                ..healthy()
            })
            .deductions()
            .first()
            .map_or(0, |d| d.points);
            assert!(points <= previous, "total {total}, free {free}");
            previous = points;
            free += step;
        }
    }
}

#[test]
fn more_memory_use_never_deducts_fewer_points() {
    let mut previous = 0;
    let mut used = 0.0_f32;
    while used <= 100.0 {
        let points = assess(&HealthInputs {
            memory_used_percent: Some(used),
            ..healthy()
        })
        .deductions()
        .first()
        .map_or(0, |d| d.points);
        assert!(points >= previous, "used {used}");
        previous = points;
        used += 0.1;
    }
}

#[test]
fn the_largest_possible_deduction_is_the_sum_of_the_biggest_of_each_check() {
    assert_eq!(MAX_TOTAL_DEDUCTION, 30 + 25 + 10 + 15);
    const {
        assert!(MAX_TOTAL_DEDUCTION < SCORE_MAX);
    }
    // The tiers of each check are ordered, so "the biggest" is the critical one.
    const {
        assert!(DISK_CRITICAL_POINTS > DISK_LOW_POINTS && DISK_LOW_POINTS > DISK_WATCH_POINTS);
    }
    const {
        assert!(MEMORY_CRITICAL_POINTS > MEMORY_HIGH_POINTS);
    }
    const {
        assert!(STARTUP_MANY_POINTS > STARTUP_SOME_POINTS);
    }
    const {
        assert!(GOOD_MIN_SCORE > FAIR_MIN_SCORE);
    }
    const {
        assert!(DISK_CRITICAL_FREE_PERCENT < DISK_LOW_FREE_PERCENT);
    }
    const {
        assert!(DISK_LOW_FREE_PERCENT < DISK_WATCH_FREE_PERCENT);
    }
}

// --- Reading the system -----------------------------------------------------------------------

const CBS: &str =
    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Component Based Servicing\\RebootPending";
const WU: &str =
    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\WindowsUpdate\\Auto Update\\RebootRequired";
const SESSION: &str = "SYSTEM\\CurrentControlSet\\Control\\Session Manager";

fn rename_value() -> RegistryValueRef {
    RegistryValueRef::new(
        RegistryHive::LocalMachine,
        SESSION,
        "PendingFileRenameOperations",
    )
}

#[test]
fn no_marker_means_no_pending_restart() {
    let registry = FakeRegistry::new();
    assert_eq!(pending_restart_markers(&registry), Ok(vec![]));
}

#[test]
fn each_marker_alone_says_a_restart_is_pending() {
    let registry = FakeRegistry::new();
    registry.add_key(RegistryHive::LocalMachine, CBS, RegistryView::Native);
    assert_eq!(
        pending_restart_markers(&registry),
        Ok(vec![RestartMarker::ComponentServicing])
    );

    let registry = FakeRegistry::new();
    registry.add_key(RegistryHive::LocalMachine, WU, RegistryView::Native);
    assert_eq!(
        pending_restart_markers(&registry),
        Ok(vec![RestartMarker::WindowsUpdate])
    );

    let registry = FakeRegistry::new();
    registry.set(
        &rename_value(),
        RegistryValue::MultiSz(vec!["\\??\\C:\\old.dll".into(), String::new()]),
    );
    assert_eq!(
        pending_restart_markers(&registry),
        Ok(vec![RestartMarker::PendingFileRename])
    );
}

#[test]
fn an_empty_or_odd_file_rename_value_is_not_a_marker() {
    for value in [
        RegistryValue::MultiSz(vec![]),
        RegistryValue::Dword(1),
        RegistryValue::Sz("x".into()),
    ] {
        let registry = FakeRegistry::new();
        registry.set(&rename_value(), value);
        assert_eq!(pending_restart_markers(&registry), Ok(vec![]));
    }
}

#[test]
fn all_markers_set_are_all_reported() {
    let registry = FakeRegistry::new();
    registry.add_key(RegistryHive::LocalMachine, CBS, RegistryView::Native);
    registry.add_key(RegistryHive::LocalMachine, WU, RegistryView::Native);
    registry.set(&rename_value(), RegistryValue::MultiSz(vec!["a".into()]));
    assert_eq!(
        pending_restart_markers(&registry),
        Ok(vec![
            RestartMarker::ComponentServicing,
            RestartMarker::WindowsUpdate,
            RestartMarker::PendingFileRename
        ])
    );
}

#[test]
fn a_marker_in_another_hive_does_not_count() {
    let registry = FakeRegistry::new();
    registry.add_key(RegistryHive::CurrentUser, CBS, RegistryView::Native);
    assert_eq!(pending_restart_markers(&registry), Ok(vec![]));
}

#[test]
fn an_unreadable_marker_is_an_error_unless_another_marker_is_set() {
    let registry = FakeRegistry::new();
    registry.fail_key_exists(
        RegistryHive::LocalMachine,
        WU,
        RegistryView::Native,
        PortError::AccessDenied("hklm".into()),
    );
    assert!(matches!(
        pending_restart_markers(&registry),
        Err(PortError::AccessDenied(_))
    ));

    // Positive evidence stands even when another marker could not be read.
    registry.add_key(RegistryHive::LocalMachine, CBS, RegistryView::Native);
    assert_eq!(
        pending_restart_markers(&registry),
        Ok(vec![RestartMarker::ComponentServicing])
    );
}

#[test]
fn reading_the_markers_never_writes() {
    let registry = FakeRegistry::new();
    registry.add_key(RegistryHive::LocalMachine, CBS, RegistryView::Native);
    let _ = pending_restart_markers(&registry);
    // The fake logs every write and delete; reading left nothing.
    assert_eq!(registry.get(&rename_value()), None);
}

fn sample(total: u64, available: u64, disks: &[(&str, u64, u64)]) -> crate::metrics::RawSample {
    let mut raw = typical_sample();
    raw.memory = MemoryStatus {
        total_physical_bytes: total,
        available_physical_bytes: available,
    };
    raw.disks = disks
        .iter()
        .map(|(mount, total, free)| crate::metrics::DiskReading {
            mount: (*mount).into(),
            total_bytes: *total,
            free_bytes: *free,
        })
        .collect();
    raw
}

#[test]
fn inspect_reads_the_system_disk_memory_and_markers_once() {
    let registry = FakeRegistry::new();
    registry.add_key(RegistryHive::LocalMachine, WU, RegistryView::Native);
    // 16 GB with 1.6 GB available is 90 percent in use. D: is fuller, but it is not the system disk.
    let metrics = FakeMetrics::constant(sample(
        16 * GB,
        16 * GB / 10,
        &[("D:\\", 1000 * GB, GB), ("C:\\", 476 * GB, 24 * GB)],
    ));

    let inputs = read_inputs(&registry, &metrics, "c:", None);
    assert_eq!(inputs.system_disk, Some(disk(476 * GB, 24 * GB)));
    assert_eq!(inputs.restart_pending, Some(true));
    assert!(
        inputs
            .memory_used_percent
            .is_some_and(|p| (p - 90.0).abs() < 0.01)
    );
    assert_eq!(metrics.reads(), 1);

    let report = inspect(&registry, &metrics, "C:\\", None);
    assert_eq!(
        ids(&report),
        ["disk.low", "memory.critical", "restart.pending"]
    );
    assert_eq!(report.score(), 100 - 20 - 25 - 10);
}

#[test]
fn a_failed_sample_leaves_disk_and_memory_unchecked_but_the_restart_is_still_read() {
    let registry = FakeRegistry::new();
    let metrics = FakeMetrics::scripted(vec![Err(PortError::Other("counters broke".into()))]);
    let report = inspect(&registry, &metrics, "C:", None);
    assert_eq!(
        report.unchecked(),
        [HealthCheck::SystemDisk, HealthCheck::Memory]
    );
    assert!(report.deductions().is_empty());
    assert_eq!(report.level(), HealthLevel::Good);
}

#[test]
fn a_missing_system_disk_is_unchecked_not_healthy() {
    let registry = FakeRegistry::new();
    let metrics = FakeMetrics::constant(sample(16 * GB, 8 * GB, &[("D:\\", 100 * GB, 50 * GB)]));
    let report = inspect(&registry, &metrics, "C:", None);
    assert_eq!(report.unchecked(), [HealthCheck::SystemDisk]);
}

#[test]
fn an_unreadable_registry_leaves_the_restart_unchecked() {
    let registry = FakeRegistry::new();
    registry.fail_key_exists(
        RegistryHive::LocalMachine,
        CBS,
        RegistryView::Native,
        PortError::AccessDenied("hklm".into()),
    );
    let metrics = FakeMetrics::constant(sample(16 * GB, 8 * GB, &[("C:\\", 500 * GB, 300 * GB)]));
    let report = inspect(&registry, &metrics, "C:", None);
    assert_eq!(report.unchecked(), [HealthCheck::PendingRestart]);
    assert_eq!(report.score(), 100);
}

#[test]
fn the_failure_log_names_the_class_and_never_the_message() {
    // `error_class` is the only text the failure paths log; a secret in the message cannot leak.
    let secret = PortError::Other("SECRET-IN-THE-MESSAGE".into());
    assert_eq!(error_class(&secret), "error");
    assert!(!error_class(&secret).contains("SECRET"));
}

#[test]
fn sizes_are_written_like_the_windows_shell_writes_them() {
    assert_eq!(format_bytes(0), "0 MB");
    assert_eq!(format_bytes(512 * MIB), "512 MB");
    assert_eq!(format_bytes(GIB), "1.0 GB");
    assert_eq!(format_bytes(24 * GIB), "24.0 GB");
    assert_eq!(format_bytes(476 * GIB), "476 GB");
    assert_eq!(format_share(5, 100), "5.0%");
    assert_eq!(format_share(24, 100), "24%");
}
