//! issue #6 验收测试：改写 prompt 注入规则 note + 反向红线复检。
//!
//! 覆盖（纯函数 + 真实 audit-draft 复检，不联网）：
//! - `rewrite_prompt`：note 优先、label 兜底、未标注兜底、文风注入 system、事实约束恒定；
//! - `apply_rewrites`：改写应用、被拒跳过、相同跳过、缺失跳过、标点吞并；
//! - `redline_check`：同文件恒等、AI 腔稿 → 干净稿的告警指标下降、
//!   `per_10k > max` 规则的 hard_flags 触发确认。

use std::io::Write;

use novel_review::tools::judge::{apply_rewrites, redline_check, rewrite_prompt, Ranked};

fn ranked(note: &str, label: &str) -> Ranked {
    Ranked {
        text: "然后他停下脚步。".to_string(),
        source: "$.samples[0].text".to_string(),
        label: label.to_string(),
        note: note.to_string(),
        probability: 0.87,
        rewrite: None,
        ai_prob_after: None,
        verify_rejected: false,
        slop_class: None,
    }
}

fn write_temp(dir: &std::path::Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).expect("创建临时文件");
    f.write_all(text.as_bytes()).expect("写临时文件");
    path
}

// ---------- rewrite_prompt：规则 note 注入 ----------

#[test]
fn rewrite_prompt_prefers_rule_note_over_label() {
    let r = ranked("流水账连接：改写用动作和场面推进，少用连接词", "然后");
    let (_, user) = rewrite_prompt(&r, None);
    assert!(user.contains("问题：流水账连接"));
    assert!(user.contains("AI 腔概率 0.87"));
    assert!(user.contains("然后他停下脚步。"));
    assert!(!user.contains("问题：然后"), "note 非空时不得退回 label");
}

#[test]
fn rewrite_prompt_falls_back_to_label_when_note_empty() {
    let r = ranked("", "然后");
    let (_, user) = rewrite_prompt(&r, None);
    assert!(user.contains("问题：然后"));
}

#[test]
fn rewrite_prompt_marks_unlabeled_when_both_empty() {
    let r = ranked("", "");
    let (_, user) = rewrite_prompt(&r, None);
    assert!(user.contains("问题：未标注"));
}

#[test]
fn rewrite_prompt_keeps_fact_constraints_in_system() {
    let r = ranked("note", "label");
    let (system, _) = rewrite_prompt(&r, None);
    assert!(system.contains("只改表达，不改事实"));
    assert!(system.contains("人名、数字、引语、关键情节不得变动"));
    assert!(system.contains("不要添加原文没有的信息"));
}

#[test]
fn rewrite_prompt_injects_style_guidance_into_system() {
    let r = ranked("note", "label");
    let guidance = "参考文风（来自样本 4 句）：平均句长约 8 字。";
    let (system, _) = rewrite_prompt(&r, Some(guidance));
    assert!(system.contains("参考文风"));
    assert!(system.ends_with(guidance));
}

#[test]
fn rewrite_prompt_without_guidance_has_no_style_section() {
    let r = ranked("note", "label");
    let (system, _) = rewrite_prompt(&r, None);
    assert!(!system.contains("参考文风"));
}

// ---------- apply_rewrites：改写后全文 ----------

#[test]
fn apply_rewrites_replaces_original_sentence() {
    let mut r = ranked("", "");
    r.rewrite = Some("他停住脚步。".to_string());
    let out = apply_rewrites("然后他停下脚步。风把门吹开。", &[r]);
    assert_eq!(out, "他停住脚步。风把门吹开。");
}

#[test]
fn apply_rewrites_skips_verify_rejected() {
    let mut r = ranked("", "");
    r.rewrite = Some("他停住脚步。".to_string());
    r.verify_rejected = true;
    let draft = "然后他停下脚步。风把门吹开。";
    assert_eq!(apply_rewrites(draft, &[r]), draft, "被拒改写不得应用");
}

#[test]
fn apply_rewrites_skips_identical_rewrite() {
    let mut r = ranked("", "");
    r.rewrite = Some("然后他停下脚步。".to_string());
    let draft = "然后他停下脚步。风把门吹开。";
    assert_eq!(apply_rewrites(draft, &[r]), draft);
}

#[test]
fn apply_rewrites_skips_missing_original() {
    let mut r = ranked("", "");
    r.text = "不存在的句子".to_string();
    r.rewrite = Some("替换文本。".to_string());
    let draft = "然后他停下脚步。";
    assert_eq!(apply_rewrites(draft, &[r]), draft);
}

#[test]
fn apply_rewrites_absorbs_duplicated_trailing_punctuation() {
    // 原文标点已在外层，改写自带句号 → 不得留下 `。。`。
    let mut r = ranked("", "");
    r.rewrite = Some("他停住脚步。".to_string());
    let out = apply_rewrites("然后他停下脚步。风把门吹开。", &[r]);
    assert!(!out.contains("。。"), "不得产生连续句号: {out}");
    assert_eq!(out, "他停住脚步。风把门吹开。");
}

#[test]
fn apply_rewrites_absorbs_trailing_quote_then_period() {
    // 改写以引号结尾、原文随后是句号 → 吞掉句号，避免 `""`/`。"` 残留堆叠。
    let mut r = ranked("", "");
    r.text = "他说：\"走吧\"".to_string();
    r.rewrite = Some("他说：\"该走了\"".to_string());
    let draft = "他说：\"走吧\"。她点头。";
    let out = apply_rewrites(draft, &[r]);
    assert_eq!(out, "他说：\"该走了\"她点头。", "应吞掉改写引号后的句号");
    assert!(!out.contains("。。"));
}

// ---------- redline_check：反向红线复检 ----------

#[test]
fn redline_check_identical_files_report_equal_metrics() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = "然后他推开那扇沉重的木门，走进雨里，任冰冷的雨水打在脸上。然后他沿着青石板的巷子往深处走去，脚步声被雨声盖住。然后他在一扇朱漆大门前停下，抬头望着门楣上的灯笼。";
    let a = write_temp(dir.path(), "a.md", text);
    let b = write_temp(dir.path(), "b.md", text);
    let (ow, oh, rw, rh) = redline_check(&a, &b, None).expect("复检应成功");
    assert_eq!((ow, oh), (rw, rh), "同文本两侧指标必须一致");
    // 3×然后 → token 规则 + 连接句式 + 同开头等多节告警。
    assert!(oh >= 3, "应触发多个 hard flag，实际 {oh}");
    assert!(ow >= 3, "应触发多个 warn 分区，实际 {ow}");
}

#[test]
fn redline_check_detects_improvement_after_rewrite() {
    let dir = tempfile::tempdir().expect("临时目录");
    // 原稿：连接词连发 + 三句同开头（AI 腔典型）。
    let sloppy = "然后他推开那扇沉重的木门，走进雨里，任冰冷的雨水打在脸上。然后他沿着青石板的巷子往深处走去，脚步声被雨声盖住。然后他在一扇朱漆大门前停下，抬头望着门楣上的灯笼。";
    // 改写后：去连接词、换句首，句长结构保持一致。
    let cleaned = "他推开那扇沉重的木门，走进雨里，任冰冷的雨水打在脸上。顺着青石板的巷子往深处走去，脚步声被雨声盖住。一扇朱漆大门拦在面前，门楣上的灯笼在雨里晃。";
    let orig = write_temp(dir.path(), "orig.md", sloppy);
    let rewritten = write_temp(dir.path(), "clean.md", cleaned);
    let (ow, oh, rw, rh) = redline_check(&orig, &rewritten, None).expect("复检应成功");
    assert!(rw < ow, "改写后 warn_sections 应下降: {ow} -> {rw}");
    assert!(rh < oh, "改写后 hard_flags 应下降: {oh} -> {rh}");
    assert_eq!((ow, oh), (4, 4), "原稿指标应稳定可复现");
    assert_eq!((rw, rh), (1, 1), "改写后指标应稳定可复现");
}

#[test]
fn redline_check_single_token_exceeds_density_threshold() {
    let dir = tempfile::tempdir().expect("临时目录");
    // 「然后」max_per_10k=8：约 30 字内 1 次即 ≈333/万字 → hard flag。
    let a = write_temp(
        dir.path(),
        "hit.md",
        "然后他推开那扇沉重的木门，走进雨里，任冰冷的雨水打在脸上。",
    );
    let b = write_temp(
        dir.path(),
        "clean.md",
        "他推开那扇沉重的木门，走进雨里，任冰冷的雨水打在脸上。",
    );
    let (_, oh, _, rh) = redline_check(&a, &b, None).expect("复检应成功");
    assert!(oh >= 2, "超密度词应额外触发 hard flag，实际 {oh}");
    assert!(oh > rh, "去掉超密度词后 flag 应更少: {oh} vs {rh}");
}
