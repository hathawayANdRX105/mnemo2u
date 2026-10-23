//! 文本拆分与计数工具的行为测试。

use novel_review::text::{prose_char_count, quote_ratio, TextSplitter};

fn splitter() -> TextSplitter {
    TextSplitter::new(r"^\s*(?:#|\||[-*]\s*状态)").expect("拆分器应可创建")
}

#[test]
fn prose_char_count_counts_cjk_and_ascii() {
    assert_eq!(prose_char_count("雷德抬头123"), 7);
    assert_eq!(prose_char_count("，。！"), 0);
    assert_eq!(prose_char_count(""), 0);
}

#[test]
fn quote_ratio_counts_quote_chars() {
    assert_eq!(quote_ratio("“你好”"), 2.0 / 4.0);
    assert_eq!(quote_ratio("你好"), 0.0);
    assert_eq!(quote_ratio(""), 0.0);
}

#[test]
fn split_sentences_splits_on_punctuation_and_newlines() {
    let s = splitter();
    assert_eq!(
        s.split_sentences("第一句。第二句！\n第三句"),
        vec![
            "第一句".to_string(),
            "第二句".to_string(),
            "第三句".to_string(),
        ]
    );
}

#[test]
fn split_sentence_infos_tracks_line_and_chars() {
    let s = splitter();
    let infos = s.split_sentence_infos("雷德抬头看了一眼。\n“走吧。”");
    assert_eq!(infos.len(), 2);
    assert_eq!(infos[0].line_no, 1);
    assert_eq!(infos[0].chars, 8);
    assert_eq!(infos[1].line_no, 2);
    assert!(infos[1].text.starts_with('“'));
}

#[test]
fn split_sentence_infos_skips_markdown_noise() {
    let s = splitter();
    let infos = s.split_sentence_infos("- 状态：待定\n正文一句。");
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].line_no, 2);
}

#[test]
fn split_paragraph_infos_groups_by_blank_line() {
    let s = splitter();
    let infos = s.split_paragraph_infos("第一段。\n第二行。\n\n第二段。");
    assert_eq!(infos.len(), 2);
    assert_eq!(infos[0].line_start, 1);
    assert_eq!(infos[0].line_end, 2);
    assert_eq!(infos[1].line_start, 4);
    assert_eq!(infos[1].line_end, 4);
}

#[test]
fn split_paragraph_infos_detects_dialogue() {
    let s = splitter();
    let infos = s.split_paragraph_infos("“你来了。”\n“嗯。”\n\n旁白一句。");
    assert!(infos[0].is_dialogue);
    assert!(!infos[1].is_dialogue);
}
