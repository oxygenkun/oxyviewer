//! Burst (continuous shooting) grouping.
//!
//! Sony cameras stamp every frame of a burst with a 1-based
//! `SequenceNumber` maker note, and record the drive mode in `ReleaseMode`.
//! Grouping is a pure function over those values: it never touches the
//! filesystem, so a caller can run it on whatever slice of a directory it has
//! already loaded. Reading the values themselves stays off the interactive
//! open path - see `docs/PERFORMANCE_INVARIANTS.md`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Frames shot seconds apart still belong to one burst; a larger gap means the
/// photographer pressed the shutter again, even if the counter kept climbing.
pub const BURST_MAX_GAP_SECONDS: i64 = 10;

/// Burst-relevant values read from one asset's metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BurstFrame {
    pub path: PathBuf,
    /// 1-based index inside the burst. `None` means the frame stands alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_mode: Option<String>,
    /// `DateTimeOriginal` as stored, for example `2026:08:24 17:42:18`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
}

impl BurstFrame {
    /// Drive modes that reuse `SequenceNumber` for something other than a
    /// burst: their frames differ by exposure, white balance or DRO, not by
    /// time, so they must not be collapsed together.
    fn is_bracketing(&self) -> bool {
        matches!(
            self.release_mode.as_deref(),
            Some("Exposure Bracketing" | "White Balance Bracketing" | "DRO Bracketing")
        )
    }

    fn takes_part_in_bursts(&self) -> bool {
        self.sequence_number.is_some() && !self.is_bracketing()
    }
}

/// A run of frames the camera shot in one burst, in shooting order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BurstGroup {
    /// First frame of the run: the tile the grid keeps visible when collapsed.
    pub representative: PathBuf,
    /// Every frame of the run, including the representative.
    pub members: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_mode: Option<String>,
}

/// Group consecutive burst frames.
///
/// `frames` is expected in browsing order. A run continues while the sequence
/// number increases by exactly one and the timestamps stay within
/// [`BURST_MAX_GAP_SECONDS`]; anything else - a missing index, a repeated index,
/// a bracketing drive mode, a jump or a long pause - ends it. Runs of a single
/// frame are dropped, because a lone frame has nothing to collapse.
pub fn group_bursts(frames: &[BurstFrame]) -> Vec<BurstGroup> {
    let mut groups: Vec<BurstGroup> = Vec::new();
    let mut run: Vec<&BurstFrame> = Vec::new();

    for frame in frames {
        if !frame.takes_part_in_bursts() {
            flush(&mut groups, &mut run);
            continue;
        }
        let Some(sequence) = frame.sequence_number else {
            continue;
        };
        let Some(last) = run.last() else {
            run.push(frame);
            continue;
        };
        // RAW+JPEG of the same shot share an index: keep the first copy.
        if last.sequence_number == Some(sequence) {
            continue;
        }
        let continues = last.sequence_number.and_then(|prev| prev.checked_add(1)) == Some(sequence)
            && within_burst_gap(last.captured_at.as_deref(), frame.captured_at.as_deref());
        if !continues {
            flush(&mut groups, &mut run);
        }
        run.push(frame);
    }
    flush(&mut groups, &mut run);
    groups
}

fn flush(groups: &mut Vec<BurstGroup>, run: &mut Vec<&BurstFrame>) {
    if run.len() < 2 {
        run.clear();
        return;
    }
    groups.push(BurstGroup {
        representative: run[0].path.clone(),
        members: run.iter().map(|frame| frame.path.clone()).collect(),
        release_mode: run[0].release_mode.clone(),
    });
    run.clear();
}

/// An unparseable or missing timestamp cannot disprove a burst, so it keeps
/// the run alive rather than splitting it.
fn within_burst_gap(previous: Option<&str>, next: Option<&str>) -> bool {
    match (
        previous.and_then(parse_exif_seconds),
        next.and_then(parse_exif_seconds),
    ) {
        (Some(previous), Some(next)) => (next - previous).abs() <= BURST_MAX_GAP_SECONDS,
        _ => true,
    }
}

/// Parse `YYYY:MM:DD HH:MM:SS` into epoch seconds. Returns `None` for the
/// empty, truncated and non-numeric values EXIF is allowed to carry.
fn parse_exif_seconds(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let number = |range: std::ops::Range<usize>| {
        std::str::from_utf8(&bytes[range]).ok()?.parse::<i64>().ok()
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days since 1970-01-01 (Howard Hinnant's civil-to-days algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(name: &str, sequence: Option<u32>, mode: Option<&str>, at: &str) -> BurstFrame {
        BurstFrame {
            path: PathBuf::from(format!("/shoot/{name}")),
            sequence_number: sequence,
            release_mode: mode.map(str::to_owned),
            captured_at: Some(at.to_owned()),
        }
    }

    fn members(groups: &[BurstGroup]) -> Vec<Vec<String>> {
        groups
            .iter()
            .map(|group| {
                group
                    .members
                    .iter()
                    .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn groups_a_continuous_run() {
        let frames = [
            frame(
                "DSC00509.ARW",
                Some(1),
                Some("Continuous"),
                "2026:08:24 17:42:18",
            ),
            frame(
                "DSC00510.ARW",
                Some(2),
                Some("Continuous"),
                "2026:08:24 17:42:18",
            ),
            frame(
                "DSC00511.HIF",
                Some(4),
                Some("Continuous"),
                "2026:08:24 17:42:19",
            ),
        ];
        let groups = group_bursts(&frames);
        assert_eq!(members(&groups), [["DSC00509.ARW", "DSC00510.ARW"]]);
        assert_eq!(
            groups[0].representative,
            PathBuf::from("/shoot/DSC00509.ARW")
        );
    }

    #[test]
    fn single_frames_are_not_groups() {
        // A lone frame - and a run whose counter never advances - collapse to
        // nothing, otherwise every single shot would become its own group.
        let frames = [
            frame(
                "DSC02948.ARW",
                Some(1),
                Some("Continuous"),
                "2026:08:24 17:42:18",
            ),
            frame(
                "DSC02949.ARW",
                Some(1),
                Some("Continuous"),
                "2026:08:24 17:42:31",
            ),
        ];
        assert!(group_bursts(&frames).is_empty());
    }

    #[test]
    fn counter_reset_starts_a_new_group() {
        let frames = [
            frame("A.ARW", Some(1), Some("Continuous"), "2026:08:24 17:42:18"),
            frame("B.ARW", Some(2), Some("Continuous"), "2026:08:24 17:42:18"),
            frame("C.ARW", Some(1), Some("Continuous"), "2026:08:24 17:43:02"),
            frame("D.ARW", Some(2), Some("Continuous"), "2026:08:24 17:43:02"),
        ];
        assert_eq!(
            members(&group_bursts(&frames)),
            [["A.ARW", "B.ARW"], ["C.ARW", "D.ARW"]]
        );
    }

    #[test]
    fn a_long_pause_splits_a_run_that_keeps_counting() {
        let frames = [
            frame("A.ARW", Some(1), Some("Continuous"), "2026:08:24 17:42:18"),
            frame("B.ARW", Some(2), Some("Continuous"), "2026:08:24 17:42:18"),
            frame("C.ARW", Some(3), Some("Continuous"), "2026:08:24 17:50:41"),
        ];
        assert_eq!(members(&group_bursts(&frames)), [["A.ARW", "B.ARW"]]);
    }

    #[test]
    fn bracketing_runs_are_left_alone() {
        let frames = [
            frame(
                "A.ARW",
                Some(1),
                Some("Exposure Bracketing"),
                "2026:08:24 17:42:18",
            ),
            frame(
                "B.ARW",
                Some(2),
                Some("Exposure Bracketing"),
                "2026:08:24 17:42:18",
            ),
            frame(
                "C.ARW",
                Some(3),
                Some("Exposure Bracketing"),
                "2026:08:24 17:42:19",
            ),
        ];
        assert!(group_bursts(&frames).is_empty());
    }

    #[test]
    fn raw_and_jpeg_of_one_shot_share_an_index() {
        let frames = [
            frame(
                "DSC001.ARW",
                Some(1),
                Some("Continuous"),
                "2026:08:24 17:42:18",
            ),
            frame(
                "DSC001.JPG",
                Some(1),
                Some("Continuous"),
                "2026:08:24 17:42:18",
            ),
            frame(
                "DSC002.ARW",
                Some(2),
                Some("Continuous"),
                "2026:08:24 17:42:18",
            ),
            frame(
                "DSC002.JPG",
                Some(2),
                Some("Continuous"),
                "2026:08:24 17:42:18",
            ),
        ];
        assert_eq!(
            members(&group_bursts(&frames)),
            [["DSC001.ARW", "DSC002.ARW"]]
        );
    }

    #[test]
    fn unparseable_timestamps_do_not_split_a_run() {
        let frames = [
            frame("A.ARW", Some(1), Some("Continuous"), ""),
            frame("B.ARW", Some(2), Some("Continuous"), "0000:00:00 00:00:00"),
        ];
        assert_eq!(members(&group_bursts(&frames)), [["A.ARW", "B.ARW"]]);
    }

    #[test]
    fn parses_exif_datetime_into_epoch_seconds() {
        assert_eq!(parse_exif_seconds("1970:01:01 00:00:00"), Some(0));
        assert_eq!(
            parse_exif_seconds("2026:08:24 17:42:18"),
            Some(1_787_593_338)
        );
        assert_eq!(parse_exif_seconds(""), None);
        assert_eq!(parse_exif_seconds("2026:08:24"), None);
        assert_eq!(parse_exif_seconds("2026:08:24 17:42:1x"), None);
    }
}
