// SPDX-License-Identifier: Apache-2.0

//! A larger exam for the support-check judge: 100 items in English,
//! Simplified Chinese, Traditional Chinese and mixed language, covering more
//! ways a sentence can go beyond its source than the 6-case qualification
//! batch. It measures how well a model judges; it does not gate anything.
//!
//! Live run (needs a cached model):
//! `WENLAN_M5_JUDGE_MODEL=qwen3.5-4b cargo test -p wenlan-core --features eval-harness \
//!   --test m5_judge_exam -- --ignored --nocapture`

use std::collections::BTreeSet;

const EXAM_BYTES: &[u8] = include_bytes!("fixtures/m5_judge_exam_v2.jsonl");
const LANGS: [&str; 4] = ["en", "zh-Hans", "zh-Hant", "mixed"];
const BATCHES: usize = 4;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ExamCase {
    case_id: String,
    category: String,
    lang: String,
    claim: String,
    span: String,
    entails: bool,
}

fn exam() -> Vec<ExamCase> {
    let text = std::str::from_utf8(EXAM_BYTES).unwrap();
    assert!(text.ends_with('\n'), "exam fixture must end with LF");
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// Item order in each batch: every 4th case, so each batch mixes languages.
fn batches(cases: &[ExamCase]) -> Vec<Vec<usize>> {
    (0..BATCHES)
        .map(|b| (b..cases.len()).step_by(BATCHES).collect())
        .collect()
}

#[test]
fn exam_fixture_is_well_formed_and_balanced() {
    let cases = exam();
    assert_eq!(cases.len(), 100);
    let ids: BTreeSet<_> = cases.iter().map(|c| c.case_id.as_str()).collect();
    assert_eq!(ids.len(), cases.len(), "case ids are unique");
    for case in &cases {
        assert!(LANGS.contains(&case.lang.as_str()), "{}", case.case_id);
        assert!(!case.claim.trim().is_empty() && !case.span.trim().is_empty());
    }
    let positives = cases.iter().filter(|c| c.entails).count();
    assert!((35..=50).contains(&positives), "positives {positives}");
    for lang in LANGS {
        assert!(cases.iter().filter(|c| c.lang == lang).count() >= 10);
    }
    let batches = batches(&cases);
    assert!(batches
        .iter()
        .all(|b| b.len() <= wenlan_core::claim_judge::M5_CLAIM_JUDGE_MAX_BATCH_ITEMS));
    assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), cases.len());
}

#[cfg(feature = "eval-harness")]
#[tokio::test]
#[ignore = "requires a cached on-device model and local inference runtime"]
async fn live_m5_judge_exam_receipt() {
    use std::collections::BTreeMap;
    use wenlan_core::claim_judge::{
        build_m5_claim_entailment_batch_user_prompt, parse_m5_claim_entailment_batch_scores,
        M5ClaimEntailmentItem, M5_CLAIM_ENTAILMENT_SYSTEM_PROMPT, M5_CLAIM_JUDGE_TIMEOUT_SECS,
        M5_JUDGE_QUALIFICATION_THRESHOLD,
    };
    use wenlan_core::llm_provider::{LlmProvider, LlmRequest, OnDeviceProvider};
    use wenlan_core::on_device_models::{get_model, is_cached};

    const MODEL_ENV: &str = "WENLAN_M5_JUDGE_MODEL";
    let model_id = std::env::var(MODEL_ENV).unwrap_or_else(|_| "qwen3.5-4b".to_string());
    let model = get_model(&model_id)
        .unwrap_or_else(|| panic!("{MODEL_ENV} must name a registered on-device model"));
    assert!(
        is_cached(model),
        "{model_id} is not in the local hf-hub cache"
    );
    let constructor_id = model_id.clone();
    let provider = tokio::task::spawn_blocking(move || {
        OnDeviceProvider::new_with_model(Some(&constructor_id))
    })
    .await
    .unwrap()
    .expect("construct cached on-device judge");

    let cases = exam();
    let mut results = Vec::new();
    let mut unparseable = Vec::new();
    let started = std::time::Instant::now();
    for (order, reversed) in [("forward", false), ("reversed", true)] {
        for (batch_index, batch) in batches(&cases).into_iter().enumerate() {
            let mut batch = batch;
            if reversed {
                batch.reverse();
            }
            // Neutral ids so the case name (which says the category) never
            // reaches the model.
            let item_ids: Vec<String> = (0..batch.len()).map(|i| format!("item-{i:02}")).collect();
            let items: Vec<_> = batch
                .iter()
                .zip(&item_ids)
                .map(|(&case_index, item_id)| M5ClaimEntailmentItem {
                    item_id,
                    claim: &cases[case_index].claim,
                    evidence: &cases[case_index].span,
                })
                .collect();
            let batch_started = std::time::Instant::now();
            let raw = provider
                .generate(LlmRequest {
                    system_prompt: Some(M5_CLAIM_ENTAILMENT_SYSTEM_PROMPT.to_string()),
                    user_prompt: build_m5_claim_entailment_batch_user_prompt(&items).unwrap(),
                    max_tokens: 2048,
                    temperature: 0.0,
                    label: Some("m5_judge_exam".to_string()),
                    timeout_secs: Some(M5_CLAIM_JUDGE_TIMEOUT_SECS),
                })
                .await;
            let seconds = batch_started.elapsed().as_secs_f64();
            let scores = raw
                .as_deref()
                .ok()
                .and_then(|raw| parse_m5_claim_entailment_batch_scores(raw, &item_ids));
            let Some(scores) = scores else {
                unparseable.push(serde_json::json!({
                    "order": order, "batch": batch_index, "seconds": seconds,
                    "raw": raw.map_err(|e| e.to_string()),
                }));
                continue;
            };
            for (score, &case_index) in scores.iter().zip(&batch) {
                let case = &cases[case_index];
                let predicted = score.score >= M5_JUDGE_QUALIFICATION_THRESHOLD;
                results.push(serde_json::json!({
                    "order": order,
                    "batch": batch_index,
                    "batch_seconds": seconds,
                    "case_id": case.case_id,
                    "category": case.category,
                    "lang": case.lang,
                    "entails": case.entails,
                    "score": score.score,
                    "correct": predicted == case.entails,
                }));
            }
        }
    }

    let tally = |key: &str| {
        let mut by: BTreeMap<String, (u32, u32)> = BTreeMap::new();
        for r in &results {
            let entry = by.entry(r[key].as_str().unwrap().to_string()).or_default();
            entry.1 += 1;
            if r["correct"].as_bool().unwrap() {
                entry.0 += 1;
            }
        }
        by.into_iter()
            .map(|(k, (ok, n))| (k, format!("{ok}/{n}")))
            .collect::<BTreeMap<_, _>>()
    };
    let correct = results
        .iter()
        .filter(|r| r["correct"].as_bool().unwrap())
        .count();
    let false_positive = results
        .iter()
        .filter(|r| !r["correct"].as_bool().unwrap() && !r["entails"].as_bool().unwrap())
        .count();
    let wrong: Vec<_> = results
        .iter()
        .filter(|r| !r["correct"].as_bool().unwrap())
        .map(|r| format!("{}:{}:{:.2}", r["order"], r["case_id"], r["score"]))
        .collect();
    let receipt = serde_json::json!({
        "schema": "wenlan-m5-judge-exam-receipt-v1",
        "model_id": model_id,
        "items_scored": results.len(),
        "items_expected": cases.len() * 2,
        "correct": correct,
        "false_positive": false_positive,
        "false_negative": results.len() - correct - false_positive,
        "unparseable_batches": unparseable,
        "by_lang": tally("lang"),
        "by_category": tally("category"),
        "wrong": wrong,
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "results": results,
    });
    println!("{}", serde_json::to_string(&receipt).unwrap());
}
