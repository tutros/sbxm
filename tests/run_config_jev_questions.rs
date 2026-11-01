//! Issue 343 (M2a slice 14b, decision 20): `[[eval.jev.questions]]` in the run-config. The
//! rubric's shape, kinds `noul`, `choice` and `score`, `options` only on a choice (2 to 255),
//! `levels` only on a score (2 to 10), unique ids, no unknown keys.

mod common;

use std::path::PathBuf;

use common::Env;
use sbxm::run::config::{QuestionKind, RunConfig};

const BASE: &str = "[task]\nprompt = \"Do it\"\n\n\
                    [[contestants]]\nharness = \"claude\"\nmodel = \"a\"\n\n\
                    [[contestants]]\nharness = \"codex\"\nmodel = \"b\"\n\n\
                    [[eval.rubric]]\nid = \"correct\"\nkind = \"pass_fail\"\nweight = 1.0\n\n\
                    [eval.jev]\ntoken_ref = \"op://v/j/t\"\n\n";

fn write(env: &Env, body: &str) -> PathBuf {
    let path = env.tmp.path().join("run.toml");
    std::fs::write(&path, body).unwrap();
    path
}

fn load_ok(questions: &str) -> RunConfig {
    let env = Env::new();
    RunConfig::load(&write(&env, &format!("{BASE}{questions}"))).unwrap()
}

/// The error, which must be one line naming the file.
fn load_err(questions: &str) -> String {
    let env = Env::new();
    let err = RunConfig::load(&write(&env, &format!("{BASE}{questions}"))).unwrap_err();
    let text = format!("{err:#}");
    assert!(!text.contains('\n'), "not one line: {text}");
    assert!(text.contains("run.toml"), "doesn't name the file: {text}");
    text
}

fn choice(n: usize) -> String {
    let options: Vec<String> = (0..n).map(|i| format!("o{i} = \"option {i}\"")).collect();
    format!(
        "[[eval.jev.questions]]\nid = \"approach\"\nkind = \"choice\"\n\
         question = \"Which approach?\"\noptions = {{ {} }}\n",
        options.join(", ")
    )
}

fn score(n: usize) -> String {
    let levels: Vec<String> = (0..n).map(|i| format!("\"l{i}\"")).collect();
    format!(
        "[[eval.jev.questions]]\nid = \"depth\"\nkind = \"score\"\n\
         question = \"How deep?\"\nlevels = [{}]\n",
        levels.join(", ")
    )
}

const NOUL: &str = "[[eval.jev.questions]]\nid = \"tests\"\nkind = \"noul\"\n\
                    question = \"Did it add tests?\"\n";

#[test]
fn the_three_kinds_load_in_order() {
    let config = load_ok(&format!(
        "{NOUL}\n[[eval.jev.questions]]\nid = \"approach\"\nkind = \"choice\"\n\
         question = \"Which approach?\"\n\
         options = {{ refactor = \"Restructured the code\", rewrite = \"Replaced it\" }}\n\n{}",
        score(3)
    ));
    let questions = config.eval.jev.unwrap().questions;
    let ids: Vec<&str> = questions.iter().map(|q| q.id.as_str()).collect();
    assert_eq!(ids, ["tests", "approach", "depth"]);
    assert_eq!(questions[0].kind, QuestionKind::Noul);
    assert_eq!(questions[0].question, "Did it add tests?");
    assert_eq!(questions[1].kind, QuestionKind::Choice);
    assert_eq!(
        questions[1].options.get("refactor").map(String::as_str),
        Some("Restructured the code")
    );
    assert_eq!(questions[1].options.len(), 2);
    assert_eq!(questions[2].kind, QuestionKind::Score);
    assert_eq!(questions[2].levels, ["l0", "l1", "l2"]);
}

#[test]
fn without_questions_there_are_none() {
    assert!(load_ok("").eval.jev.unwrap().questions.is_empty());
}

#[test]
fn a_choice_with_2_or_255_options_loads() {
    for n in [2, 255] {
        let config = load_ok(&choice(n));
        assert_eq!(config.eval.jev.unwrap().questions[0].options.len(), n);
    }
}

#[test]
fn a_choice_with_1_or_256_options_is_refused() {
    for n in [1, 256] {
        let err = load_err(&choice(n));
        assert!(
            err.contains("eval.jev.questions[0]") && err.contains("approach"),
            "{n}: {err}"
        );
        assert!(err.contains(&format!("{n} options")), "{n}: {err}");
    }
}

#[test]
fn a_score_with_2_or_10_levels_loads() {
    for n in [2, 10] {
        let config = load_ok(&score(n));
        assert_eq!(config.eval.jev.unwrap().questions[0].levels.len(), n);
    }
}

#[test]
fn a_score_with_1_or_11_levels_is_refused() {
    for n in [1, 11] {
        let err = load_err(&score(n));
        assert!(
            err.contains("eval.jev.questions[0]") && err.contains("depth"),
            "{n}: {err}"
        );
        assert!(err.contains(&format!("{n} distinct levels")), "{n}: {err}");
    }
}

#[test]
fn repeated_levels_count_once() {
    let err = load_err(
        "[[eval.jev.questions]]\nid = \"depth\"\nkind = \"score\"\nquestion = \"q\"\n\
         levels = [\"a\", \"a\"]\n",
    );
    assert!(err.contains("1 distinct levels"), "{err}");
}

#[test]
fn options_on_another_kind_are_refused() {
    for kind in ["noul", "score"] {
        let levels = if kind == "score" {
            "levels = [\"a\", \"b\"]\n"
        } else {
            ""
        };
        let err = load_err(&format!(
            "[[eval.jev.questions]]\nid = \"q1\"\nkind = \"{kind}\"\nquestion = \"q\"\n{levels}\
             options = {{ a = \"A\", b = \"B\" }}\n"
        ));
        assert!(
            err.contains("eval.jev.questions[0]") && err.contains("q1") && err.contains("options"),
            "{kind}: {err}"
        );
    }
}

#[test]
fn levels_on_another_kind_are_refused() {
    for kind in ["noul", "choice"] {
        let options = if kind == "choice" {
            "options = { a = \"A\", b = \"B\" }\n"
        } else {
            ""
        };
        let err = load_err(&format!(
            "[[eval.jev.questions]]\nid = \"q1\"\nkind = \"{kind}\"\nquestion = \"q\"\n{options}\
             levels = [\"a\", \"b\"]\n"
        ));
        assert!(
            err.contains("eval.jev.questions[0]") && err.contains("q1") && err.contains("levels"),
            "{kind}: {err}"
        );
    }
}

#[test]
fn a_choice_without_options_is_refused() {
    let err = load_err(
        "[[eval.jev.questions]]\nid = \"approach\"\nkind = \"choice\"\nquestion = \"q\"\n",
    );
    assert!(
        err.contains("eval.jev.questions[0]")
            && err.contains("approach")
            && err.contains("options"),
        "{err}"
    );
}

#[test]
fn a_score_without_levels_is_refused() {
    let err =
        load_err("[[eval.jev.questions]]\nid = \"depth\"\nkind = \"score\"\nquestion = \"q\"\n");
    assert!(
        err.contains("eval.jev.questions[0]") && err.contains("depth") && err.contains("levels"),
        "{err}"
    );
}

#[test]
fn an_unknown_key_is_refused() {
    let err = load_err(&format!("{NOUL}weight = 1.0\n"));
    assert!(err.contains("weight"), "{err}");
}

#[test]
fn an_unknown_kind_is_refused() {
    let err =
        load_err("[[eval.jev.questions]]\nid = \"q1\"\nkind = \"pass_fail\"\nquestion = \"q\"\n");
    assert!(err.contains("pass_fail"), "{err}");
}

#[test]
fn a_duplicate_question_id_is_refused() {
    let err = load_err(&format!("{NOUL}\n{NOUL}"));
    assert!(
        err.contains("tests") && err.contains("eval.jev.questions[1]"),
        "{err}"
    );
}

#[test]
fn a_question_id_that_is_a_rubric_criterions_is_refused() {
    // Both are keys of the same `questions` object of one request.
    let err =
        load_err("[[eval.jev.questions]]\nid = \"correct\"\nkind = \"noul\"\nquestion = \"q\"\n");
    assert!(
        err.contains("eval.jev.questions[0]") && err.contains("correct") && err.contains("rubric"),
        "{err}"
    );
}

#[test]
fn an_empty_question_is_refused() {
    let err = load_err("[[eval.jev.questions]]\nid = \"q1\"\nkind = \"noul\"\nquestion = \" \"\n");
    assert!(
        err.contains("eval.jev.questions[0]") && err.contains("question"),
        "{err}"
    );
}

#[test]
fn an_empty_option_id_or_description_is_refused() {
    for options in ["{ \"\" = \"A\", b = \"B\" }", "{ a = \" \", b = \"B\" }"] {
        let err = load_err(&format!(
            "[[eval.jev.questions]]\nid = \"approach\"\nkind = \"choice\"\nquestion = \"q\"\n\
             options = {options}\n"
        ));
        assert!(
            err.contains("eval.jev.questions[0]") && err.contains("option"),
            "{options}: {err}"
        );
    }
}
