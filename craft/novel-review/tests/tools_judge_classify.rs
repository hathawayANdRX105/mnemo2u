//! `tools.judge` 单元/契约测试：`--classify` 请求构造与 voice matching 文风画像。
//!
//! 只测纯函数（不联网）：classify 的 catalog 完备性、question/请求体结构、
//! style guidance 的句长/短句/连接词特征判定。

use novel_review::tools::judge::{
    build_classify_request, classify_question, style_guidance_from_text, CLASSIFY_CLASSES,
};

/// catalog 互斥且含 normal 兜底类。
#[test]
fn classify_catalog_has_normal_catchall() {
    let ids: Vec<&str> = CLASSIFY_CLASSES.iter().map(|(id, _)| *id).collect();
    assert!(ids.contains(&"normal"), "目录必须含 normal 兜底类");
    assert_eq!(ids.len(), 7, "目录应有 7 个类别");
    // 每个 id 唯一。
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "类别 id 必须唯一");
    // 每个类别的描述都带「不是什么」或边界说明（目录质量要求）。
    for (id, description) in CLASSIFY_CLASSES {
        assert!(
            description.contains("不是") || description.contains("不算") || id == "normal",
            "类别 {id} 的描述应写清排除边界"
        );
    }
}

/// classify_question 是 choice 类型、criteria 覆盖全部类别且含 margin/auto_accept。
#[test]
fn classify_question_shape() {
    let q = classify_question(0, "这是一个强者对弱者的俯视");
    assert_eq!(q["type"], "choice");
    assert!(q["instructions"]
        .as_str()
        .unwrap()
        .contains("这是一个强者对弱者的俯视"));
    let criteria = q["criteria"].as_object().expect("criteria 应为对象");
    assert_eq!(criteria.len(), CLASSIFY_CLASSES.len());
    for (id, _) in CLASSIFY_CLASSES {
        assert!(criteria.contains_key(id), "criteria 缺少类别 {id}");
    }
    assert_eq!(q["minimum_margin"], 0.5);
    assert_eq!(q["auto_accept"], 0.85);
}

/// 请求体：questions 与 items 按 s{index} 对齐，state 带 purpose/items/classes。
#[test]
fn build_classify_request_alignment() {
    let texts = vec![
        "不是不想走，而是不能走".to_string(),
        "他把门推开，把灯打开".to_string(),
    ];
    let body = build_classify_request("judge-latest", &texts);
    assert_eq!(body["model"], "judge-latest");
    let questions = body["questions"].as_object().expect("questions 应为对象");
    assert_eq!(questions.len(), 2);
    assert!(questions.contains_key("s0"));
    assert!(questions.contains_key("s1"));
    let items = body["state"]["items"].as_array().expect("items 应为数组");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], "s0");
    assert_eq!(items[1]["id"], "s1");
    assert_eq!(items[0]["text"], "不是不想走，而是不能走");
    let classes = body["state"]["classes"]
        .as_array()
        .expect("classes 应为数组");
    assert_eq!(classes.len(), CLASSIFY_CLASSES.len());
    assert_eq!(
        body["state"]["purpose"].as_str().unwrap().is_empty(),
        false.to_owned()
    );
}

/// 空候选不产生问题。
#[test]
fn build_classify_request_empty() {
    let body = build_classify_request("judge-latest", &[]);
    assert_eq!(body["questions"].as_object().unwrap().len(), 0);
}

/// 短句为主的样本：guidance 报「以短句为主」。
#[test]
fn style_guidance_short_sentences() {
    let sample = "他走。他停。他抬头。他进门。灯灭。风起。";
    let g = style_guidance_from_text(sample);
    assert!(g.contains("短句"), "应报告短句占比: {g}");
    assert!(g.contains("以短句为主"), "短句过半应提示多用紧凑短句: {g}");
}

/// 长句为主的样本：guidance 报「长短句交错」。
#[test]
fn style_guidance_long_sentences() {
    let sample = "他在雨里走了很久，久到路灯一盏盏亮起来，又一盏盏灭下去，街道尽头传来货车的轰鸣，而他只是站着，像一尊被遗忘的石像。";
    let g = style_guidance_from_text(sample);
    assert!(g.contains("长短句交错"), "长句样本不应判定短句为主: {g}");
}

/// 连接词稀疏样本：走中性"具体呈现"分支（无连接词堆叠可言）。
#[test]
fn style_guidance_connective_free() {
    let sample = "他站定。雨落。远处传来钟声，一下，又一下。他转身，走入巷子深处的黑暗里。";
    let g = style_guidance_from_text(sample);
    assert!(g.contains("具体呈现"), "无连接词样本应走具体呈现分支: {g}");
    assert!(
        !g.contains("避免堆叠"),
        "无连接词样本不应出现堆叠避免提示: {g}"
    );
}

/// 连接词密集样本：提示避免堆叠连接词。
#[test]
fn style_guidance_connective_heavy() {
    let sample = "然后他走。然后他停。然后他抬头。接着他叹了口气，于是转身离去。";
    let g = style_guidance_from_text(sample);
    assert!(g.contains("避免堆叠"), "连接词密集样本应提示避免堆叠: {g}");
}

/// 空样本退化为中性指导。
#[test]
fn style_guidance_empty_fallback() {
    let g = style_guidance_from_text("   \n  ");
    assert!(g.contains("中性叙述"));
}
