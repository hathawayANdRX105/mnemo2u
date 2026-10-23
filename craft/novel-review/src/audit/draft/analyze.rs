//! `analyze_text` 前半装配：规则指标与各结构节 → 中间节 `AnalysisSections`。

use super::*;

// ---------------------------------------------------------------------------
// analyze_text / analyze_path / CLI

/// 报告输出格式（对齐 `--format`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    /// 纯文本报告。
    Text,
    /// JSON 报告。
    Json,
    /// Markdown 报告。
    Markdown,
}

/// 语料画像 JSON 节。
pub(crate) fn corpus_profile_json(corpus: Option<&CorpusProfile>) -> CorpusProfileJson {
    let to_json = |item: &LearnedPattern| LearnedTermJson {
        name: item.name.clone(),
        category: item.category.clone(),
        count: item.count,
        corpus_per_10k: item.corpus_per_10k,
        max_per_10k: item.max_per_10k,
    };
    match corpus {
        Some(c) => CorpusProfileJson {
            enabled: true,
            source_count: c.source_count,
            chars: c.chars,
            draft_chars: c.draft_chars,
            learned_terms: c.learned_terms.iter().map(to_json).collect(),
            learned_style_phrases: c.learned_style_phrases.iter().map(to_json).collect(),
            learned_sentence_leads: c.learned_sentence_leads.clone(),
            learned_aa_bb_shapes: c.learned_aa_bb_shapes.clone(),
            sentence_length_baseline: match &c.sentence_length_baseline {
                Some(baseline) => BaselineJson::Values(baseline.clone()),
                None => BaselineJson::Empty,
            },
        },
        None => CorpusProfileJson {
            enabled: false,
            source_count: 0,
            chars: 0,
            draft_chars: 0,
            learned_terms: Vec::new(),
            learned_style_phrases: Vec::new(),
            learned_sentence_leads: Vec::new(),
            learned_aa_bb_shapes: Vec::new(),
            sentence_length_baseline: BaselineJson::Empty,
        },
    }
}

/// 指标证据（首条样本 snippet；无样本时为空串）。
pub(crate) fn first_snippet(samples: &[Hit]) -> String {
    samples
        .first()
        .map(|s| s.snippet.clone())
        .unwrap_or_default()
}

/// `analyze_text` 前半产物：装配 `Analysis` 所需的全部中间量
/// （体与原 `analyze_text` 逐行一致；由 `assemble_analysis` 消费）。
pub(crate) struct AnalysisSections<'a> {
    /// 文本。
    pub text: &'a str,
    /// 语料画像（借用）。
    pub corpus_profile: Option<&'a CorpusProfile>,
    pub chars: usize,
    pub sentence_infos: Vec<crate::text::SentenceInfo>,
    pub paragraphs: Vec<String>,
    pub token_metrics: Vec<RegexMetric>,
    pub pattern_metrics: Vec<RegexMetric>,
    pub phrase_metrics: Vec<RegexMetric>,
    pub modifier_metrics: Vec<RegexMetric>,
    pub punctuation_metrics: Vec<RegexMetric>,
    pub combo_metrics: Vec<RegexMetric>,
    pub token_warn: bool,
    pub pattern_warn: bool,
    pub phrase_warn: bool,
    pub modifier_warn: bool,
    pub punctuation_warn: bool,
    pub combo_warn: bool,
    pub custom_template_metrics: Vec<CustomTemplateMetric>,
    pub custom_warn: bool,
    pub tracked_term_metrics: Vec<crate::rules::TrackedMetric>,
    pub tracked_term_categories: Vec<crate::rules::CategoryRow>,
    pub tracked_term_warn: bool,
    pub learned_filter_metrics: Vec<LearnedFilterMetric>,
    pub learned_filter_warn: bool,
    pub repeated_starts: Vec<PhraseCount>,
    pub subject_leads_rows: Vec<PhraseCount>,
    pub paragraph_leads_rows: Vec<PhraseCount>,
    pub sentence_patterns: Vec<PhraseCount>,
    pub judgement_endings: Vec<PhraseCount>,
    pub clause_prefixes: Vec<PhraseCount>,
    pub parallel_clauses: Vec<PhraseCount>,
    pub aa_bb_patterns: Vec<AaBbPattern>,
    pub aa_bb_warn: bool,
    pub ba_operation_contexts: Vec<BaContext>,
    pub ba_operation_context_warn: bool,
    pub modifier_pressure: Vec<ModifierPressure>,
    pub sentence_lengths: SentenceLengths,
    pub fatigue_windows: Vec<FatigueWindow>,
    pub fatigue_window_count: usize,
    pub judgement_contexts: Vec<JudgementContext>,
    pub judgement_context_warn: bool,
    pub dominant_punctuation: Vec<DominantPunctuation>,
    pub terms: Vec<TermCount>,
    pub short_phrases: Vec<TermCount>,
    pub dialogue: DialogueReport,
    pub dialogue_warn: bool,
    pub scene_map: SceneMap,
    pub dialogue_emotions: DialogueEmotions,
    pub character_voice: CharacterVoice,
    pub tone_profile: ToneProfile,
    pub battle_profile: BattleProfile,
    pub viewpoint_profile: ViewpointProfile,
    pub ending: Ending,
    pub ending_warn: bool,
    pub tracked_term_windows: Vec<TrackedTermWindow>,
    pub tracked_term_window_count: usize,
    pub template_candidates: Vec<TemplateCandidate>,
}

/// `analyze_text` 前半：计算全部规则指标与结构节（与原 `analyze_text` 逐行一致），
/// 产出 `AnalysisSections`。
pub(crate) fn analyze_sections<'a>(
    ctx: &DraftContext,
    text: &'a str,
    template_bank: &[TemplateRule],
    term_bank: &[TrackedTerm],
    corpus_profile: Option<&'a CorpusProfile>,
    sample_limit: usize,
) -> Result<AnalysisSections<'a>> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let splitter = ctx.splitter();
    let sentences = splitter.split_sentences(text);
    let sentence_infos = splitter.split_sentence_infos(text);
    let paragraph_infos = splitter.split_paragraph_infos(text);
    let paragraphs: Vec<String> = text
        .split("\n\n")
        .filter(|para| !para.trim().is_empty())
        .map(str::to_string)
        .collect();
    let chars = code_len(&text.replace('\n', ""));
    let quote_runs = detect_dialogue_runs(text);
    let short_quote_runs = detect_short_dialogue_runs(text);
    let question_ping_pong = detect_question_ping_pong(text);
    let quote_ping_pong = detect_quote_ping_pong(text);
    let dialogue_axis_gaps = build_dialogue_axis_gaps(ctx, &sentence_infos, sample_limit * 2);
    let ab_turns = detect_a_b_turns(ctx, text);

    let (token_metrics, token_warn) =
        build_rule_metrics(&ctx.token_rules, &lines, chars, false, sample_limit);
    let (pattern_metrics, pattern_warn) =
        build_rule_metrics(&ctx.pattern_rules, &lines, chars, true, sample_limit);
    let (phrase_metrics, phrase_warn) =
        build_rule_metrics(&ctx.phrase_rules, &lines, chars, false, sample_limit);
    let (modifier_metrics, modifier_warn) =
        build_rule_metrics(&ctx.modifier_rules, &lines, chars, false, sample_limit);
    let (punctuation_metrics, punctuation_warn) =
        build_rule_metrics(&ctx.punctuation_rules, &lines, chars, false, sample_limit);
    let (combo_metrics, combo_warn) =
        build_rule_metrics(&ctx.combo_rules, &lines, chars, false, sample_limit);
    let (custom_template_metrics, custom_warn) =
        build_custom_template_metrics(template_bank, &lines, chars, sample_limit)?;
    let (tracked_term_metrics, tracked_term_categories, tracked_term_warn) =
        build_tracked_term_metrics(term_bank, &lines, chars, sample_limit)?;
    let learned_filter_metrics =
        build_learned_filter_metrics(corpus_profile, &lines, chars, sample_limit);
    let learned_filter_warn = learned_filter_metrics.iter().any(|m| m.warn);

    let sentence_starts = collect_sentence_starts(&sentences);
    let subject_leads = collect_subject_leads(ctx, &sentences);
    let paragraph_leads = collect_paragraph_leads(ctx, &paragraphs);
    let repeated_starts: Vec<PhraseCount> = sentence_starts
        .into_iter()
        .filter(|(_, count)| *count >= 3)
        .map(|(phrase, count)| PhraseCount { phrase, count })
        .collect();
    let subject_leads_rows: Vec<PhraseCount> = subject_leads
        .into_iter()
        .map(|(phrase, count)| PhraseCount { phrase, count })
        .collect();
    let paragraph_leads_rows: Vec<PhraseCount> = paragraph_leads
        .into_iter()
        .map(|(phrase, count)| PhraseCount { phrase, count })
        .collect();
    let sentence_patterns_all = collect_connective_sentence_patterns(ctx, &sentences);
    let sentence_patterns: Vec<PhraseCount> = sentence_patterns_all
        .iter()
        .filter(|(_, count)| *count >= 3)
        .map(|(phrase, count)| PhraseCount {
            phrase: phrase.clone(),
            count: *count,
        })
        .collect();
    let judgement_endings: Vec<PhraseCount> = collect_judgement_endings(ctx, &sentences)
        .into_iter()
        .map(|(phrase, count)| PhraseCount { phrase, count })
        .collect();
    let clause_prefixes: Vec<PhraseCount> = collect_clause_prefixes(ctx, &sentences)
        .into_iter()
        .map(|(phrase, count)| PhraseCount { phrase, count })
        .collect();
    let parallel_clauses: Vec<PhraseCount> = collect_parallel_clauses(ctx, &sentences)
        .into_iter()
        .map(|(phrase, count)| PhraseCount { phrase, count })
        .collect();
    let aa_bb_patterns = collect_aa_bb_patterns(ctx, &sentences, sample_limit);
    let aa_bb_warn = aa_bb_patterns.iter().any(|p| p.warn);
    let ba_operation_contexts = build_ba_operation_contexts(ctx, &sentence_infos, sample_limit);
    let ba_operation_context_warn = ba_operation_contexts.iter().any(|c| c.warn);
    let modifier_pressure = collect_modifier_pressure(ctx, &sentences);
    let sentence_lengths = build_sentence_length_profile(ctx, &sentence_infos);
    let fatigue_windows = build_fatigue_windows(ctx, &sentence_infos, sample_limit * 2);
    let fatigue_window_count = fatigue_windows
        .first()
        .map(|w| w.total_candidates)
        .unwrap_or(0);
    let judgement_contexts = collect_judgement_contexts(ctx, &sentence_infos, sample_limit);
    let judgement_context_warn = judgement_contexts.iter().any(|c| c.warn);
    let scene_map = build_scene_map(ctx, &paragraph_infos, sample_limit);
    let dialogue_emotions = build_dialogue_emotion_profile(ctx, &sentence_infos, sample_limit);
    let character_voice = build_character_voice_profile(ctx, &sentence_infos, sample_limit);
    let tone_profile = build_tone_profile(ctx, &paragraph_infos, sample_limit);
    let battle_profile = build_battle_profile(ctx, &sentence_infos, sample_limit);
    let viewpoint_profile = build_viewpoint_profile(ctx, &paragraph_infos, sample_limit);
    let tracked_term_windows = build_tracked_term_windows(
        ctx,
        term_bank,
        corpus_profile
            .map(|c| c.learned_terms.as_slice())
            .unwrap_or(&[]),
        &sentence_infos,
        sample_limit * 2,
    );
    let tracked_term_window_count = tracked_term_windows
        .first()
        .map(|w| w.total_candidates)
        .unwrap_or(0);
    let terms: Vec<TermCount> = collect_ngram_terms(ctx, text, &[(2, 8), (3, 5), (4, 4)], false)
        .into_iter()
        .map(|(term, count)| TermCount { term, count })
        .collect();
    let short_phrases: Vec<TermCount> =
        collect_ngram_terms(ctx, text, &[(2, 6), (3, 5), (4, 4)], true)
            .into_iter()
            .map(|(term, count)| TermCount { term, count })
            .collect();

    let mut dominant_punctuation: Vec<DominantPunctuation> = punctuation_metrics
        .iter()
        .filter(|m| m.count > 0)
        .map(|m| DominantPunctuation {
            mark: m.name.clone(),
            count: m.count,
            per_10k: m.per_10k,
        })
        .collect();
    dominant_punctuation.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.mark.cmp(&b.mark)));

    let dialogue = DialogueReport {
        consecutive_quote_paragraph_runs: quote_runs
            .iter()
            .map(|(start, end, sample)| QuoteRun {
                start_paragraph: *start,
                end_paragraph: *end,
                sample: sample.clone(),
            })
            .collect(),
        short_quote_runs: short_quote_runs
            .iter()
            .map(|(start, end, avg_len, sample)| ShortQuoteRun {
                start_paragraph: *start,
                end_paragraph: *end,
                avg_len: *avg_len,
                sample: sample.clone(),
            })
            .collect(),
        question_ping_pong: question_ping_pong
            .iter()
            .map(|(start, end, sample)| QuoteRun {
                start_paragraph: *start,
                end_paragraph: *end,
                sample: sample.clone(),
            })
            .collect(),
        quote_ping_pong: quote_ping_pong
            .iter()
            .map(|(start, end, avg_len, sample)| ShortQuoteRun {
                start_paragraph: *start,
                end_paragraph: *end,
                avg_len: *avg_len,
                sample: sample.clone(),
            })
            .collect(),
        dialogue_axis_gaps,
        alternating_speaker_runs: ab_turns,
        quote_paragraph_ratio: round4f(quote_runs.len() as f64 / paragraphs.len().max(1) as f64),
        dense_quote_run_max: quote_runs
            .iter()
            .map(|(start, end, _)| end - start + 1)
            .max()
            .unwrap_or(0),
        dense_quote_run_count: quote_runs
            .iter()
            .filter(|(start, end, _)| end - start + 1 >= 5)
            .count(),
    };
    let dialogue_warn = !dialogue.consecutive_quote_paragraph_runs.is_empty()
        || !dialogue.alternating_speaker_runs.is_empty()
        || !dialogue.dialogue_axis_gaps.is_empty();

    let lex = ctx.lexicon();
    let tail_text = tail_chars(text, 180).to_string();
    let ending_images: Vec<TermCount> = lex
        .ending_image_terms
        .iter()
        .filter(|term| tail_text.contains(term.as_str()))
        .map(|term| TermCount {
            term: term.clone(),
            count: tail_text.matches(term.as_str()).count(),
        })
        .collect();
    let ending_flows: Vec<TermCount> = lex
        .ending_flow_terms
        .iter()
        .filter(|term| tail_text.contains(term.as_str()))
        .map(|term| TermCount {
            term: term.clone(),
            count: tail_text.matches(term.as_str()).count(),
        })
        .collect();
    let ending_warn = ending_images.len() >= 3 || ending_flows.len() >= 2;
    let ending = Ending {
        tail_excerpt: tail_text.trim().to_string(),
        image_terms: ending_images,
        flow_terms: ending_flows,
        warn: ending_warn,
    };

    // template_candidates（顺序按固定契约）。
    let mut template_candidates: Vec<TemplateCandidate> = Vec::new();
    for m in &tracked_term_metrics {
        if m.warn {
            template_candidates.push(TemplateCandidate {
                candidate_type: "tracked_term".to_string(),
                name: m.name.clone(),
                count: m.count,
                note: m.note.clone(),
                sample: first_snippet(&m.samples),
            });
        }
    }
    for (section, metrics) in [
        ("tokens", &token_metrics),
        ("patterns", &pattern_metrics),
        ("phrases", &phrase_metrics),
        ("modifiers", &modifier_metrics),
        ("punctuation", &punctuation_metrics),
        ("punctuation_combo", &combo_metrics),
    ] {
        for m in metrics {
            if m.warn {
                template_candidates.push(TemplateCandidate {
                    candidate_type: section.to_string(),
                    name: m.name.clone(),
                    count: m.count,
                    note: m.note.clone(),
                    sample: first_snippet(&m.samples),
                });
            }
        }
    }
    for m in &custom_template_metrics {
        if m.warn {
            template_candidates.push(TemplateCandidate {
                candidate_type: "custom_template".to_string(),
                name: m.name.clone(),
                count: m.count,
                note: m.note.clone(),
                sample: first_snippet(&m.samples),
            });
        }
    }
    for m in &learned_filter_metrics {
        if m.warn {
            template_candidates.push(TemplateCandidate {
                candidate_type: "learned_filter".to_string(),
                name: m.name.clone(),
                count: m.count,
                note: m.note.clone(),
                sample: first_snippet(&m.samples),
            });
        }
    }
    for item in &sentence_patterns {
        template_candidates.push(TemplateCandidate {
            candidate_type: "sentence_pattern".to_string(),
            name: item.phrase.clone(),
            count: item.count,
            note: "句首骨架重复".to_string(),
            sample: String::new(),
        });
    }
    for item in &aa_bb_patterns {
        if item.warn {
            template_candidates.push(TemplateCandidate {
                candidate_type: "aa_bb_pattern".to_string(),
                name: item.name.clone(),
                count: item.count,
                note: item.note.clone(),
                sample: item.samples.first().cloned().unwrap_or_default(),
            });
        }
    }
    for item in &ba_operation_contexts {
        if !item.warn {
            continue;
        }
        let sample = item
            .samples
            .first()
            .map(|s| s.sentence.clone())
            .unwrap_or_default();
        template_candidates.push(TemplateCandidate {
            candidate_type: "ba_operation_context".to_string(),
            name: item.role.clone(),
            count: item.count,
            note: format!("把字句类型偏密；{}", item.suggestion),
            sample,
        });
    }
    if sentence_lengths.warn {
        let sample = sentence_lengths
            .short_sentences
            .first()
            .map(|s| format!("L{} {}字：{}", s.line_no, s.chars, s.text))
            .unwrap_or_default();
        template_candidates.push(TemplateCandidate {
            candidate_type: "sentence_length".to_string(),
            name: "短句密度".to_string(),
            count: sentence_lengths.short_count,
            note: "短句过多或连发，会让草稿像节拍器或对白录音".to_string(),
            sample,
        });
    }
    for item in short_phrases.iter().take(5) {
        template_candidates.push(TemplateCandidate {
            candidate_type: "short_phrase".to_string(),
            name: item.term.clone(),
            count: item.count,
            note: "短语手感重复".to_string(),
            sample: String::new(),
        });
    }
    if let Some(first) = dialogue.consecutive_quote_paragraph_runs.first() {
        template_candidates.push(TemplateCandidate {
            candidate_type: "dialogue".to_string(),
            name: "连续短对白".to_string(),
            count: dialogue.consecutive_quote_paragraph_runs.len(),
            note: "A/B 乒乓过长".to_string(),
            sample: first.sample.join(" | "),
        });
    }
    if let Some(first) = dialogue.short_quote_runs.first() {
        template_candidates.push(TemplateCandidate {
            candidate_type: "dialogue".to_string(),
            name: "短句对白块".to_string(),
            count: dialogue.short_quote_runs.len(),
            note: "对白短句过密，容易写成互答录音".to_string(),
            sample: first.sample.join(" | "),
        });
    }
    if let Some(first) = dialogue.quote_ping_pong.first() {
        template_candidates.push(TemplateCandidate {
            candidate_type: "dialogue".to_string(),
            name: "对白乒乓".to_string(),
            count: dialogue.quote_ping_pong.len(),
            note: "纯对白互顶过长，缺少动作或场面转轴".to_string(),
            sample: first.sample.join(" | "),
        });
    }
    if let Some(first_gap) = dialogue.dialogue_axis_gaps.first() {
        template_candidates.push(TemplateCandidate {
            candidate_type: "dialogue_axis_gap".to_string(),
            name: "对白转轴缺口".to_string(),
            count: dialogue.dialogue_axis_gaps.len(),
            note: format!(
                "连续对白缺少动作、环境、第三方或设备转轴；{}",
                first_gap.suggestion
            ),
            sample: first_gap
                .sample
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | "),
        });
    }
    if scene_map.warn {
        let first_block = scene_map.blocks.first();
        template_candidates.push(TemplateCandidate {
            candidate_type: "scene_map".to_string(),
            name: "场面功能失衡".to_string(),
            count: scene_map.block_count,
            note: format!(
                "粗分块里 `{}` 占比偏高，检查这章是否长时间停在同一种叙事功能里。",
                scene_map.dominant_role
            ),
            sample: first_block
                .map(|b| {
                    b.sample
                        .iter()
                        .take(2)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .unwrap_or_default(),
        });
    }
    if dialogue_emotions.flatness_warn || dialogue_emotions.volatility_warn {
        let first_sample = dialogue_emotions.samples.first();
        template_candidates.push(TemplateCandidate {
            candidate_type: "dialogue_emotion".to_string(),
            name: "对白情绪单一/横跳".to_string(),
            count: if dialogue_emotions.shift_count > 0 {
                dialogue_emotions.shift_count
            } else {
                dialogue_emotions.dialogue_sentences
            },
            note: format!(
                "dominant={} shift={}，检查对白是否只在重复顶回去。",
                dialogue_emotions.dominant_emotion, dialogue_emotions.shift_count
            ),
            sample: first_sample.map(|s| s.text.clone()).unwrap_or_default(),
        });
    }
    if character_voice.warn {
        let first_speaker = character_voice.speakers.first();
        template_candidates.push(TemplateCandidate {
            candidate_type: "character_voice".to_string(),
            name: "角色对白同质化".to_string(),
            count: character_voice.speaker_count,
            note: "多名角色的对白节拍、问句率和情绪主导过近，检查是否越来越像同一个人在说话。"
                .to_string(),
            sample: first_speaker.map(|s| s.speaker.clone()).unwrap_or_default(),
        });
    }
    if battle_profile.warn {
        let first_sample = battle_profile.samples.first();
        template_candidates.push(TemplateCandidate {
            candidate_type: "battle_profile".to_string(),
            name: "动作链缺结果".to_string(),
            count: battle_profile.sequence_count,
            note: "动作/冲突段有推进，但结果句、受伤反馈或位移后果不足，容易只剩挥打。".to_string(),
            sample: first_sample
                .map(|s| {
                    s.sample
                        .iter()
                        .take(4)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .unwrap_or_default(),
        });
    }
    if viewpoint_profile.warn {
        let first_overlap = viewpoint_profile.overlaps.first();
        template_candidates.push(TemplateCandidate {
            candidate_type: "viewpoint_profile".to_string(),
            name: "视角锚点漂移".to_string(),
            count: if viewpoint_profile.overlap_count > 0 {
                viewpoint_profile.overlap_count
            } else {
                viewpoint_profile.switch_count
            },
            note: "同段多人物心理暴露或近距离视角反复换锚，读者容易丢当前镜头中心。".to_string(),
            sample: first_overlap.map(|o| o.text.clone()).unwrap_or_default(),
        });
    }
    if ending_warn {
        let sample_source = ending.tail_excerpt.replace('\n', " ");
        template_candidates.push(TemplateCandidate {
            candidate_type: "ending".to_string(),
            name: "章末模板".to_string(),
            count: ending.image_terms.len() + ending.flow_terms.len(),
            note: "章末意象或流程词偏密，检查是否又在模板化收尾".to_string(),
            sample: prefix_chars(sample_source.as_str(), 80),
        });
    }
    if let Some(first_window) = fatigue_windows.first() {
        template_candidates.push(TemplateCandidate {
            candidate_type: "fatigue_window".to_string(),
            name: "局部疲劳窗口".to_string(),
            count: fatigue_windows.len(),
            note: "短句、判断、把字句、角色起手等问题在局部连续叠加".to_string(),
            sample: first_window
                .sample
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | "),
        });
    }
    if let Some(first_window) = tracked_term_windows.first() {
        template_candidates.push(TemplateCandidate {
            candidate_type: "tracked_term_window".to_string(),
            name: "点名局部密度".to_string(),
            count: tracked_term_windows.len(),
            note: format!(
                "同一名词或同类跟踪词在局部窗口内密集出现；{}",
                format_tracked_term_counts(
                    &first_window.terms[..first_window.terms.len().min(3)],
                    "，"
                )
            ),
            sample: first_window
                .sample
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | "),
        });
    }
    for item in &judgement_contexts {
        if !item.warn {
            continue;
        }
        let sample = item
            .samples
            .first()
            .map(|s| format!("L{} {}", s.line_no, s.text))
            .unwrap_or_default();
        template_candidates.push(TemplateCandidate {
            candidate_type: "judgement_context".to_string(),
            name: item.label.clone(),
            count: item.count,
            note: "旁白判断句偏密，容易替场面下结论".to_string(),
            sample,
        });
    }
    Ok(AnalysisSections {
        text,
        corpus_profile,
        chars,
        sentence_infos,
        paragraphs,
        token_metrics,
        pattern_metrics,
        phrase_metrics,
        modifier_metrics,
        punctuation_metrics,
        combo_metrics,
        token_warn,
        pattern_warn,
        phrase_warn,
        modifier_warn,
        punctuation_warn,
        combo_warn,
        custom_template_metrics,
        custom_warn,
        tracked_term_metrics,
        tracked_term_categories,
        tracked_term_warn,
        learned_filter_metrics,
        learned_filter_warn,
        repeated_starts,
        subject_leads_rows,
        paragraph_leads_rows,
        sentence_patterns,
        judgement_endings,
        clause_prefixes,
        parallel_clauses,
        aa_bb_patterns,
        aa_bb_warn,
        ba_operation_contexts,
        ba_operation_context_warn,
        modifier_pressure,
        sentence_lengths,
        fatigue_windows,
        fatigue_window_count,
        judgement_contexts,
        judgement_context_warn,
        dominant_punctuation,
        terms,
        short_phrases,
        dialogue,
        dialogue_warn,
        scene_map,
        dialogue_emotions,
        character_voice,
        tone_profile,
        battle_profile,
        viewpoint_profile,
        ending,
        ending_warn,
        tracked_term_windows,
        tracked_term_window_count,
        template_candidates,
    })
}

/// 装配全部顶层节并计算 warned/warn_sections。
pub fn analyze_text(
    ctx: &DraftContext,
    text: &str,
    source: &str,
    template_bank: &[TemplateRule],
    term_bank: &[TrackedTerm],
    corpus_profile: Option<&CorpusProfile>,
    sample_limit: usize,
) -> Result<Analysis> {
    let sections = analyze_sections(
        ctx,
        text,
        template_bank,
        term_bank,
        corpus_profile,
        sample_limit,
    )?;
    assemble_analysis(sections, source)
}

/// 读文件文本后走 `analyze_text`（source 为路径字符串）。
pub fn analyze_path(
    ctx: &DraftContext,
    path: &Path,
    template_bank: &[TemplateRule],
    term_bank: &[TrackedTerm],
    corpus_profile: Option<&CorpusProfile>,
    sample_limit: usize,
) -> Result<Analysis> {
    let text =
        fs::read_to_string(path).with_context(|| format!("无法读取文件 {}", path.display()))?;
    analyze_text(
        ctx,
        &text,
        &path.display().to_string(),
        template_bank,
        term_bank,
        corpus_profile,
        sample_limit,
    )
}
