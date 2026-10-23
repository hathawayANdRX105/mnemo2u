//! story 内跨章合流快照（`build_story_trend_snapshots`）。

use super::*;

use super::axes::ending_display;

/// 同值连续段（`collect_repeated_value_runs` 行）。
#[derive(Debug, Clone)]
struct RepeatedRun {
    value: String,
    paths: Vec<PathBuf>,
}

/// `collect_repeated_value_runs`：按 `chapter_sort_key` 排序后收 ≥ min_run 的同值段。
fn collect_repeated_value_runs(rows: &[(PathBuf, &str)], min_run: usize) -> Vec<RepeatedRun> {
    let mut ordered: Vec<(PathBuf, &str)> = rows.to_vec();
    ordered.sort_by_key(|a| chapter_sort_key(&a.0));
    let mut runs: Vec<RepeatedRun> = Vec::new();
    let mut current_value: Option<&str> = None;
    let mut current_paths: Vec<PathBuf> = Vec::new();
    let flush = |runs: &mut Vec<RepeatedRun>,
                 current_value: &mut Option<&str>,
                 current_paths: &mut Vec<PathBuf>| {
        if let Some(value) = *current_value {
            if current_paths.len() >= min_run {
                runs.push(RepeatedRun {
                    value: value.to_string(),
                    paths: current_paths.clone(),
                });
            }
        }
        *current_value = None;
        current_paths.clear();
    };
    for (draft_path, value) in &ordered {
        let value = *value;
        if Some(value) == current_value {
            current_paths.push(draft_path.clone());
            continue;
        }
        flush(&mut runs, &mut current_value, &mut current_paths);
        current_value = Some(value);
        current_paths.push(draft_path.clone());
    }
    flush(&mut runs, &mut current_value, &mut current_paths);
    runs
}

/// story 内跨章合流快照（`convergence_kinds` + `notes`）。
#[derive(Debug, Clone, Default)]
pub struct TrendSnapshot {
    pub convergence_kinds: Vec<String>,
    pub notes: Vec<String>,
}

/// `build_story_trend_snapshots`：章末标签连续段 × 色调/情绪同值段重叠。
#[must_use]
pub fn build_story_trend_snapshots(
    analyses: &[(PathBuf, &Analysis)],
    labels: &EndingLabels,
) -> Vec<(PathBuf, TrendSnapshot)> {
    let mut ordered: Vec<(PathBuf, &Analysis)> = analyses.to_vec();
    ordered.sort_by_key(|a| chapter_sort_key(&a.0));

    let mut ending_runs: Vec<(String, Vec<PathBuf>)> = Vec::new();
    let mut current_label: Option<String> = None;
    let mut current_paths: Vec<PathBuf> = Vec::new();
    for (draft_path, analysis) in &ordered {
        let label = infer_ending_label(analysis, labels);
        if Some(label.as_str()) == current_label.as_deref() {
            current_paths.push(draft_path.clone());
            continue;
        }
        if let Some(label) = current_label.take() {
            if current_paths.len() >= 2 {
                ending_runs.push((label, current_paths.clone()));
            }
        }
        current_label = Some(label);
        current_paths.clear();
        current_paths.push(draft_path.clone());
    }
    if let Some(label) = current_label {
        if current_paths.len() >= 2 {
            ending_runs.push((label, current_paths));
        }
    }

    let tone_runs = collect_repeated_value_runs(
        &ordered
            .iter()
            .map(|(path, analysis)| {
                let dominant = &analysis.tone_profile.dominant_tone;
                (
                    path.clone(),
                    if dominant.is_empty() {
                        "none"
                    } else {
                        dominant
                    },
                )
            })
            .collect::<Vec<_>>(),
        2,
    );
    let emotion_runs = collect_repeated_value_runs(
        &ordered
            .iter()
            .map(|(path, analysis)| {
                let dominant = &analysis.dialogue_emotions.dominant_emotion;
                (
                    path.clone(),
                    if dominant.is_empty() {
                        "neutral"
                    } else {
                        dominant
                    },
                )
            })
            .collect::<Vec<_>>(),
        2,
    );

    fn snapshot_entry<'a>(
        snapshots: &'a mut Vec<(PathBuf, TrendSnapshot)>,
        path: &PathBuf,
    ) -> &'a mut TrendSnapshot {
        if let Some(i) = snapshots.iter().position(|(p, _)| p == path) {
            &mut snapshots[i].1
        } else {
            snapshots.push((path.clone(), TrendSnapshot::default()));
            &mut snapshots.last_mut().expect("刚插入").1
        }
    }
    let mut snapshots: Vec<(PathBuf, TrendSnapshot)> = Vec::new();
    for (ending_label, ending_paths) in &ending_runs {
        let ending_set: HashSet<&PathBuf> = ending_paths.iter().collect();
        let ending_display_name = ending_display(labels, ending_label);
        for tone_run in &tone_runs {
            if tone_run.value == "none" {
                continue;
            }
            let overlap: Vec<&PathBuf> = tone_run
                .paths
                .iter()
                .filter(|p| ending_set.contains(p))
                .collect();
            if overlap.len() < 2 {
                continue;
            }
            for path in overlap {
                let snapshot = snapshot_entry(&mut snapshots, path);
                snapshot.convergence_kinds.push("ending_tone".to_string());
                snapshot
                    .notes
                    .push(format!("{ending_display_name}+tone:{}", tone_run.value));
            }
        }
        for emotion_run in &emotion_runs {
            if emotion_run.value == "neutral" {
                continue;
            }
            let overlap: Vec<&PathBuf> = emotion_run
                .paths
                .iter()
                .filter(|p| ending_set.contains(p))
                .collect();
            if overlap.len() < 2 {
                continue;
            }
            for path in overlap {
                let snapshot = snapshot_entry(&mut snapshots, path);
                snapshot
                    .convergence_kinds
                    .push("ending_emotion".to_string());
                snapshot.notes.push(format!(
                    "{ending_display_name}+emotion:{}",
                    emotion_run.value
                ));
            }
        }
    }
    snapshots
}
